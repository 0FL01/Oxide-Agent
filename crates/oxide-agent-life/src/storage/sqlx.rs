//! SQLx/Postgres implementation for life-mode source-of-truth storage.

use std::path::Path;

use async_trait::async_trait;
use sqlx_core::migrate::Migrator;
use sqlx_core::query::query;
use sqlx_core::row::Row;
use sqlx_postgres::{PgPool, Postgres};
use uuid::Uuid;

use crate::domain::{
    ActiveMemoryGeneration, FrictionPatternId, FrictionPatternKind, LifeFrictionPattern,
    LifeIdentityLink, LifeIdentityProvider, LifeMemoryGeneration, LifeMemoryItem, LifePrincipal,
    LifeSupportProtocol, LifeTaskState, MemoryAuthority, MemoryGenerationId,
    MemoryGenerationStatus, MemoryItemId, MemoryItemKind, MemoryItemStatus, MemoryScope,
    MemorySensitivity, PrincipalUserId, ProviderSubject, SupportProtocolId, SupportStateStatus,
    TaskStateId, TaskStateStatus, TimestampMillis, TurnId,
};
use crate::storage::{LifeStorageError, LifeStorageRepository, LifeStorageResult};

/// SQLx-backed life storage repository.
#[derive(Clone)]
pub struct SqlxLifeStorage {
    pool: PgPool,
}

impl SqlxLifeStorage {
    /// Creates a storage repository from a shared Postgres pool.
    #[must_use]
    pub const fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Returns the shared pool.
    #[must_use]
    pub const fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Runs migrations from a filesystem path.
    pub async fn run_migrations_from_path(&self, path: impl AsRef<Path>) -> LifeStorageResult<()> {
        let migrator = Migrator::new(path.as_ref())
            .await
            .map_err(|error| LifeStorageError::Migration(error.to_string()))?;
        migrator
            .run(&self.pool)
            .await
            .map_err(|error| LifeStorageError::Migration(error.to_string()))
    }

    async fn ensure_user_row_in_tx(
        tx: &mut sqlx_core::transaction::Transaction<'_, Postgres>,
        principal_user_id: PrincipalUserId,
    ) -> LifeStorageResult<()> {
        query::<Postgres>("INSERT INTO users (user_id) VALUES ($1) ON CONFLICT DO NOTHING")
            .bind(principal_user_id.get())
            .execute(&mut **tx)
            .await
            .map_err(db_error)?;
        Ok(())
    }

    async fn generation_belongs_to_principal_in_tx(
        tx: &mut sqlx_core::transaction::Transaction<'_, Postgres>,
        principal_user_id: PrincipalUserId,
        memory_generation_id: MemoryGenerationId,
    ) -> LifeStorageResult<bool> {
        let row = query::<Postgres>(
            r#"
            SELECT 1
            FROM life_memory_generations
            WHERE principal_user_id = $1 AND memory_generation_id = $2
            "#,
        )
        .bind(principal_user_id.get())
        .bind(memory_generation_id.as_uuid())
        .fetch_optional(&mut **tx)
        .await
        .map_err(db_error)?;
        Ok(row.is_some())
    }
}

#[async_trait]
impl LifeStorageRepository for SqlxLifeStorage {
    async fn upsert_principal(&self, principal: &LifePrincipal) -> LifeStorageResult<()> {
        let mut tx = self.pool.begin().await.map_err(db_error)?;
        Self::ensure_user_row_in_tx(&mut tx, principal.principal_user_id).await?;

        query::<Postgres>(
            r#"
            INSERT INTO life_principals (
                principal_user_id, profile_state, operating_profile, settings,
                schema_version, created_at, updated_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            ON CONFLICT (principal_user_id) DO UPDATE
            SET profile_state = EXCLUDED.profile_state,
                operating_profile = EXCLUDED.operating_profile,
                settings = EXCLUDED.settings,
                schema_version = EXCLUDED.schema_version,
                updated_at = EXCLUDED.updated_at
            "#,
        )
        .bind(principal.principal_user_id.get())
        .bind(&principal.profile_state)
        .bind(&principal.operating_profile)
        .bind(&principal.settings)
        .bind(principal.schema_version)
        .bind(principal.created_at.get())
        .bind(principal.updated_at.get())
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;

        tx.commit().await.map_err(db_error)
    }

    async fn link_identity(&self, link: &LifeIdentityLink) -> LifeStorageResult<()> {
        query::<Postgres>(
            r#"
            INSERT INTO life_identity_links (
                provider, provider_subject, principal_user_id, verified_at, created_at, updated_at
            )
            VALUES ($1, $2, $3, $4, $5, $6)
            ON CONFLICT (provider, provider_subject) DO UPDATE
            SET principal_user_id = EXCLUDED.principal_user_id,
                verified_at = EXCLUDED.verified_at,
                updated_at = EXCLUDED.updated_at
            "#,
        )
        .bind(link.provider.as_str())
        .bind(link.provider_subject.as_str())
        .bind(link.principal_user_id.get())
        .bind(link.verified_at.map(TimestampMillis::get))
        .bind(link.created_at.get())
        .bind(link.updated_at.get())
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(())
    }

    async fn resolve_identity(
        &self,
        provider: LifeIdentityProvider,
        provider_subject: &ProviderSubject,
    ) -> LifeStorageResult<Option<PrincipalUserId>> {
        let row = query::<Postgres>(
            r#"
            SELECT principal_user_id
            FROM life_identity_links
            WHERE provider = $1 AND provider_subject = $2
            "#,
        )
        .bind(provider.as_str())
        .bind(provider_subject.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?;

        row.map(|row| PrincipalUserId::new(row.get::<i64, _>("principal_user_id")))
            .transpose()
            .map_err(Into::into)
    }

    async fn insert_memory_generation(
        &self,
        generation: &LifeMemoryGeneration,
    ) -> LifeStorageResult<()> {
        query::<Postgres>(
            r#"
            INSERT INTO life_memory_generations (
                memory_generation_id, principal_user_id, generation_number, status,
                source_generation_id, build_reason, build_policy, source_scope,
                comparison_report, activated_at, created_at, updated_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
            ON CONFLICT (memory_generation_id) DO UPDATE
            SET status = EXCLUDED.status,
                build_policy = EXCLUDED.build_policy,
                source_scope = EXCLUDED.source_scope,
                comparison_report = EXCLUDED.comparison_report,
                activated_at = EXCLUDED.activated_at,
                updated_at = EXCLUDED.updated_at
            "#,
        )
        .bind(generation.memory_generation_id.as_uuid())
        .bind(generation.principal_user_id.get())
        .bind(generation.generation_number)
        .bind(generation_status_as_str(generation.status))
        .bind(
            generation
                .source_generation_id
                .map(MemoryGenerationId::as_uuid),
        )
        .bind(&generation.build_reason)
        .bind(&generation.build_policy)
        .bind(&generation.source_scope)
        .bind(&generation.comparison_report)
        .bind(generation.activated_at.map(TimestampMillis::get))
        .bind(generation.created_at.get())
        .bind(generation.updated_at.get())
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(())
    }

    async fn activate_memory_generation(
        &self,
        principal_user_id: PrincipalUserId,
        memory_generation_id: MemoryGenerationId,
        activated_at: TimestampMillis,
        activation_reason: &str,
    ) -> LifeStorageResult<ActiveMemoryGeneration> {
        let mut tx = self.pool.begin().await.map_err(db_error)?;
        if !Self::generation_belongs_to_principal_in_tx(
            &mut tx,
            principal_user_id,
            memory_generation_id,
        )
        .await?
        {
            return Err(LifeStorageError::GenerationNotOwned {
                principal_user_id,
                generation_id: memory_generation_id,
            });
        }

        query::<Postgres>(
            r#"
            UPDATE life_memory_generations
            SET status = 'archived', updated_at = $2
            WHERE principal_user_id = $1 AND status = 'active'
            "#,
        )
        .bind(principal_user_id.get())
        .bind(activated_at.get())
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;

        query::<Postgres>(
            r#"
            UPDATE life_memory_generations
            SET status = 'active', activated_at = $3, updated_at = $3
            WHERE principal_user_id = $1 AND memory_generation_id = $2
            "#,
        )
        .bind(principal_user_id.get())
        .bind(memory_generation_id.as_uuid())
        .bind(activated_at.get())
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;

        query::<Postgres>(
            r#"
            INSERT INTO life_active_memory_generations (
                principal_user_id, memory_generation_id, activated_at, activation_reason
            )
            VALUES ($1, $2, $3, $4)
            ON CONFLICT (principal_user_id) DO UPDATE
            SET memory_generation_id = EXCLUDED.memory_generation_id,
                activated_at = EXCLUDED.activated_at,
                activation_reason = EXCLUDED.activation_reason
            "#,
        )
        .bind(principal_user_id.get())
        .bind(memory_generation_id.as_uuid())
        .bind(activated_at.get())
        .bind(activation_reason)
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;

        tx.commit().await.map_err(db_error)?;
        Ok(ActiveMemoryGeneration {
            scope: MemoryScope::new(principal_user_id, memory_generation_id),
            activated_at,
        })
    }

    async fn active_generation(
        &self,
        principal_user_id: PrincipalUserId,
    ) -> LifeStorageResult<Option<ActiveMemoryGeneration>> {
        let row = query::<Postgres>(
            r#"
            SELECT memory_generation_id, activated_at
            FROM life_active_memory_generations
            WHERE principal_user_id = $1
            "#,
        )
        .bind(principal_user_id.get())
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?;

        Ok(row.map(|row| ActiveMemoryGeneration {
            scope: MemoryScope::new(
                principal_user_id,
                MemoryGenerationId::from_uuid(row.get::<Uuid, _>("memory_generation_id")),
            ),
            activated_at: TimestampMillis::new(row.get::<i64, _>("activated_at")),
        }))
    }

    async fn upsert_memory_item(&self, item: &LifeMemoryItem) -> LifeStorageResult<()> {
        query::<Postgres>(
            r#"
            INSERT INTO life_memory_items (
                memory_id, principal_user_id, memory_generation_id, kind, authority, status,
                text, structured, tags, evidence_turn_ids, sensitivity, valid_from, valid_to,
                supersedes_memory_id, created_at, updated_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)
            ON CONFLICT (memory_id) DO UPDATE
            SET status = EXCLUDED.status,
                text = EXCLUDED.text,
                structured = EXCLUDED.structured,
                tags = EXCLUDED.tags,
                evidence_turn_ids = EXCLUDED.evidence_turn_ids,
                sensitivity = EXCLUDED.sensitivity,
                valid_from = EXCLUDED.valid_from,
                valid_to = EXCLUDED.valid_to,
                supersedes_memory_id = EXCLUDED.supersedes_memory_id,
                updated_at = EXCLUDED.updated_at
            "#,
        )
        .bind(item.memory_id.as_uuid())
        .bind(item.principal_user_id.get())
        .bind(item.memory_generation_id.as_uuid())
        .bind(memory_item_kind_as_str(item.kind))
        .bind(memory_authority_as_str(item.authority))
        .bind(memory_item_status_as_str(item.status))
        .bind(&item.text)
        .bind(&item.structured)
        .bind(&item.tags)
        .bind(turn_ids_to_uuids(&item.evidence_turn_ids))
        .bind(memory_sensitivity_as_str(item.sensitivity))
        .bind(item.valid_from.map(TimestampMillis::get))
        .bind(item.valid_to.map(TimestampMillis::get))
        .bind(item.supersedes_memory_id.map(MemoryItemId::as_uuid))
        .bind(item.created_at.get())
        .bind(item.updated_at.get())
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(())
    }

    async fn active_memory_items(
        &self,
        scope: MemoryScope,
    ) -> LifeStorageResult<Vec<LifeMemoryItem>> {
        let rows = query::<Postgres>(
            r#"
            SELECT *
            FROM life_memory_items
            WHERE principal_user_id = $1
              AND memory_generation_id = $2
              AND status = 'active'
            ORDER BY updated_at DESC, memory_id ASC
            "#,
        )
        .bind(scope.principal_user_id.get())
        .bind(scope.memory_generation_id.as_uuid())
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;

        rows.into_iter().map(memory_item_from_row).collect()
    }

    async fn upsert_task_state(&self, task_state: &LifeTaskState) -> LifeStorageResult<()> {
        query::<Postgres>(
            r#"
            INSERT INTO life_task_states (
                task_state_id, principal_user_id, memory_generation_id, project_key,
                current_goal, why, current_state, next_action, open_loops, blockers,
                status, last_turn_id, created_at, updated_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
            ON CONFLICT (principal_user_id, memory_generation_id, project_key) DO UPDATE
            SET current_goal = EXCLUDED.current_goal,
                why = EXCLUDED.why,
                current_state = EXCLUDED.current_state,
                next_action = EXCLUDED.next_action,
                open_loops = EXCLUDED.open_loops,
                blockers = EXCLUDED.blockers,
                status = EXCLUDED.status,
                last_turn_id = EXCLUDED.last_turn_id,
                updated_at = EXCLUDED.updated_at
            "#,
        )
        .bind(task_state.task_state_id.as_uuid())
        .bind(task_state.principal_user_id.get())
        .bind(task_state.memory_generation_id.as_uuid())
        .bind(&task_state.project_key)
        .bind(&task_state.current_goal)
        .bind(&task_state.why)
        .bind(&task_state.current_state)
        .bind(&task_state.next_action)
        .bind(&task_state.open_loops)
        .bind(&task_state.blockers)
        .bind(task_state_status_as_str(task_state.status))
        .bind(task_state.last_turn_id.map(TurnId::as_uuid))
        .bind(task_state.created_at.get())
        .bind(task_state.updated_at.get())
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(())
    }

    async fn active_task_states(
        &self,
        scope: MemoryScope,
    ) -> LifeStorageResult<Vec<LifeTaskState>> {
        let rows = query::<Postgres>(
            r#"
            SELECT *
            FROM life_task_states
            WHERE principal_user_id = $1
              AND memory_generation_id = $2
              AND status IN ('active', 'paused')
            ORDER BY updated_at DESC, project_key ASC
            "#,
        )
        .bind(scope.principal_user_id.get())
        .bind(scope.memory_generation_id.as_uuid())
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;

        rows.into_iter().map(task_state_from_row).collect()
    }

    async fn upsert_friction_pattern(
        &self,
        pattern: &LifeFrictionPattern,
    ) -> LifeStorageResult<()> {
        query::<Postgres>(
            r#"
            INSERT INTO life_friction_patterns (
                pattern_id, principal_user_id, memory_generation_id, kind,
                trigger_descriptor, preferred_response, evidence_turn_ids, authority,
                status, created_at, updated_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
            ON CONFLICT (pattern_id) DO UPDATE
            SET kind = EXCLUDED.kind,
                trigger_descriptor = EXCLUDED.trigger_descriptor,
                preferred_response = EXCLUDED.preferred_response,
                evidence_turn_ids = EXCLUDED.evidence_turn_ids,
                authority = EXCLUDED.authority,
                status = EXCLUDED.status,
                updated_at = EXCLUDED.updated_at
            "#,
        )
        .bind(pattern.pattern_id.as_uuid())
        .bind(pattern.principal_user_id.get())
        .bind(pattern.memory_generation_id.as_uuid())
        .bind(friction_pattern_kind_as_str(pattern.kind))
        .bind(&pattern.trigger_descriptor)
        .bind(&pattern.preferred_response)
        .bind(turn_ids_to_uuids(&pattern.evidence_turn_ids))
        .bind(memory_authority_as_str(pattern.authority))
        .bind(support_state_status_as_str(pattern.status))
        .bind(pattern.created_at.get())
        .bind(pattern.updated_at.get())
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(())
    }

    async fn active_friction_patterns(
        &self,
        scope: MemoryScope,
    ) -> LifeStorageResult<Vec<LifeFrictionPattern>> {
        let rows = query::<Postgres>(
            r#"
            SELECT *
            FROM life_friction_patterns
            WHERE principal_user_id = $1
              AND memory_generation_id = $2
              AND status = 'active'
            ORDER BY updated_at DESC, pattern_id ASC
            "#,
        )
        .bind(scope.principal_user_id.get())
        .bind(scope.memory_generation_id.as_uuid())
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;

        rows.into_iter().map(friction_pattern_from_row).collect()
    }

    async fn upsert_support_protocol(
        &self,
        protocol: &LifeSupportProtocol,
    ) -> LifeStorageResult<()> {
        query::<Postgres>(
            r#"
            INSERT INTO life_support_protocols (
                protocol_id, principal_user_id, memory_generation_id, name,
                trigger_descriptor, steps, priority, evidence_turn_ids, authority,
                status, created_at, updated_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
            ON CONFLICT (protocol_id) DO UPDATE
            SET name = EXCLUDED.name,
                trigger_descriptor = EXCLUDED.trigger_descriptor,
                steps = EXCLUDED.steps,
                priority = EXCLUDED.priority,
                evidence_turn_ids = EXCLUDED.evidence_turn_ids,
                authority = EXCLUDED.authority,
                status = EXCLUDED.status,
                updated_at = EXCLUDED.updated_at
            "#,
        )
        .bind(protocol.protocol_id.as_uuid())
        .bind(protocol.principal_user_id.get())
        .bind(protocol.memory_generation_id.as_uuid())
        .bind(&protocol.name)
        .bind(&protocol.trigger_descriptor)
        .bind(&protocol.steps)
        .bind(protocol.priority)
        .bind(turn_ids_to_uuids(&protocol.evidence_turn_ids))
        .bind(memory_authority_as_str(protocol.authority))
        .bind(support_state_status_as_str(protocol.status))
        .bind(protocol.created_at.get())
        .bind(protocol.updated_at.get())
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(())
    }

    async fn active_support_protocols(
        &self,
        scope: MemoryScope,
    ) -> LifeStorageResult<Vec<LifeSupportProtocol>> {
        let rows = query::<Postgres>(
            r#"
            SELECT *
            FROM life_support_protocols
            WHERE principal_user_id = $1
              AND memory_generation_id = $2
              AND status = 'active'
            ORDER BY priority DESC, updated_at DESC, protocol_id ASC
            "#,
        )
        .bind(scope.principal_user_id.get())
        .bind(scope.memory_generation_id.as_uuid())
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;

        rows.into_iter().map(support_protocol_from_row).collect()
    }
}

fn db_error(error: sqlx_core::error::Error) -> LifeStorageError {
    LifeStorageError::Database(error.to_string())
}

fn generation_status_as_str(status: MemoryGenerationStatus) -> &'static str {
    match status {
        MemoryGenerationStatus::Building => "building",
        MemoryGenerationStatus::Active => "active",
        MemoryGenerationStatus::Archived => "archived",
        MemoryGenerationStatus::Failed => "failed",
        MemoryGenerationStatus::Deleted => "deleted",
    }
}

fn memory_item_kind_as_str(kind: MemoryItemKind) -> &'static str {
    match kind {
        MemoryItemKind::Biography => "biography",
        MemoryItemKind::Preference => "preference",
        MemoryItemKind::ProjectPrinciple => "project_principle",
        MemoryItemKind::Procedure => "procedure",
        MemoryItemKind::Decision => "decision",
        MemoryItemKind::Episode => "episode",
        MemoryItemKind::OperatingRule => "operating_rule",
        MemoryItemKind::FrictionPattern => "friction_pattern",
        MemoryItemKind::SupportProtocol => "support_protocol",
    }
}

fn memory_item_kind_from_str(value: &str) -> LifeStorageResult<MemoryItemKind> {
    match value {
        "biography" => Ok(MemoryItemKind::Biography),
        "preference" => Ok(MemoryItemKind::Preference),
        "project_principle" => Ok(MemoryItemKind::ProjectPrinciple),
        "procedure" => Ok(MemoryItemKind::Procedure),
        "decision" => Ok(MemoryItemKind::Decision),
        "episode" => Ok(MemoryItemKind::Episode),
        "operating_rule" => Ok(MemoryItemKind::OperatingRule),
        "friction_pattern" => Ok(MemoryItemKind::FrictionPattern),
        "support_protocol" => Ok(MemoryItemKind::SupportProtocol),
        other => unknown_enum("memory_item_kind", other),
    }
}

fn memory_authority_as_str(authority: MemoryAuthority) -> &'static str {
    match authority {
        MemoryAuthority::UserAsserted => "user_asserted",
        MemoryAuthority::UserConfirmed => "user_confirmed",
        MemoryAuthority::CuratorSuggested => "curator_suggested",
        MemoryAuthority::SystemDerived => "system_derived",
    }
}

fn memory_authority_from_str(value: &str) -> LifeStorageResult<MemoryAuthority> {
    match value {
        "user_asserted" => Ok(MemoryAuthority::UserAsserted),
        "user_confirmed" => Ok(MemoryAuthority::UserConfirmed),
        "curator_suggested" => Ok(MemoryAuthority::CuratorSuggested),
        "system_derived" => Ok(MemoryAuthority::SystemDerived),
        other => unknown_enum("memory_authority", other),
    }
}

fn memory_item_status_as_str(status: MemoryItemStatus) -> &'static str {
    match status {
        MemoryItemStatus::Active => "active",
        MemoryItemStatus::Superseded => "superseded",
        MemoryItemStatus::Deleted => "deleted",
        MemoryItemStatus::Candidate => "candidate",
    }
}

fn memory_item_status_from_str(value: &str) -> LifeStorageResult<MemoryItemStatus> {
    match value {
        "active" => Ok(MemoryItemStatus::Active),
        "superseded" => Ok(MemoryItemStatus::Superseded),
        "deleted" => Ok(MemoryItemStatus::Deleted),
        "candidate" => Ok(MemoryItemStatus::Candidate),
        other => unknown_enum("memory_item_status", other),
    }
}

fn memory_sensitivity_as_str(sensitivity: MemorySensitivity) -> &'static str {
    match sensitivity {
        MemorySensitivity::Clean => "clean",
        MemorySensitivity::Personal => "personal",
        MemorySensitivity::Redacted => "redacted",
        MemorySensitivity::SecretBlocked => "secret_blocked",
    }
}

fn memory_sensitivity_from_str(value: &str) -> LifeStorageResult<MemorySensitivity> {
    match value {
        "clean" => Ok(MemorySensitivity::Clean),
        "personal" => Ok(MemorySensitivity::Personal),
        "redacted" => Ok(MemorySensitivity::Redacted),
        "secret_blocked" => Ok(MemorySensitivity::SecretBlocked),
        other => unknown_enum("memory_sensitivity", other),
    }
}

fn task_state_status_as_str(status: TaskStateStatus) -> &'static str {
    match status {
        TaskStateStatus::Active => "active",
        TaskStateStatus::Paused => "paused",
        TaskStateStatus::Completed => "completed",
        TaskStateStatus::Abandoned => "abandoned",
    }
}

fn task_state_status_from_str(value: &str) -> LifeStorageResult<TaskStateStatus> {
    match value {
        "active" => Ok(TaskStateStatus::Active),
        "paused" => Ok(TaskStateStatus::Paused),
        "completed" => Ok(TaskStateStatus::Completed),
        "abandoned" => Ok(TaskStateStatus::Abandoned),
        other => unknown_enum("task_state_status", other),
    }
}

fn friction_pattern_kind_as_str(kind: FrictionPatternKind) -> &'static str {
    match kind {
        FrictionPatternKind::OverloadTrigger => "overload_trigger",
        FrictionPatternKind::TaskInitiationBarrier => "task_initiation_barrier",
        FrictionPatternKind::ContextLoss => "context_loss",
        FrictionPatternKind::CommunicationMismatch => "communication_mismatch",
        FrictionPatternKind::SensoryOrEnergyConstraint => "sensory_or_energy_constraint",
    }
}

fn friction_pattern_kind_from_str(value: &str) -> LifeStorageResult<FrictionPatternKind> {
    match value {
        "overload_trigger" => Ok(FrictionPatternKind::OverloadTrigger),
        "task_initiation_barrier" => Ok(FrictionPatternKind::TaskInitiationBarrier),
        "context_loss" => Ok(FrictionPatternKind::ContextLoss),
        "communication_mismatch" => Ok(FrictionPatternKind::CommunicationMismatch),
        "sensory_or_energy_constraint" => Ok(FrictionPatternKind::SensoryOrEnergyConstraint),
        other => unknown_enum("friction_pattern_kind", other),
    }
}

fn support_state_status_as_str(status: SupportStateStatus) -> &'static str {
    match status {
        SupportStateStatus::Active => "active",
        SupportStateStatus::Superseded => "superseded",
        SupportStateStatus::Deleted => "deleted",
        SupportStateStatus::Candidate => "candidate",
    }
}

fn support_state_status_from_str(value: &str) -> LifeStorageResult<SupportStateStatus> {
    match value {
        "active" => Ok(SupportStateStatus::Active),
        "superseded" => Ok(SupportStateStatus::Superseded),
        "deleted" => Ok(SupportStateStatus::Deleted),
        "candidate" => Ok(SupportStateStatus::Candidate),
        other => unknown_enum("support_state_status", other),
    }
}

fn unknown_enum<T>(type_name: &'static str, value: &str) -> LifeStorageResult<T> {
    Err(LifeStorageError::UnknownEnumValue {
        type_name,
        value: value.to_owned(),
    })
}

fn turn_ids_to_uuids(turn_ids: &[TurnId]) -> Vec<Uuid> {
    turn_ids.iter().map(|id| id.as_uuid()).collect()
}

fn uuids_to_turn_ids(uuids: Vec<Uuid>) -> Vec<TurnId> {
    uuids.into_iter().map(TurnId::from_uuid).collect()
}

fn memory_item_from_row(row: sqlx_postgres::PgRow) -> LifeStorageResult<LifeMemoryItem> {
    Ok(LifeMemoryItem {
        memory_id: MemoryItemId::from_uuid(row.get("memory_id")),
        principal_user_id: PrincipalUserId::new(row.get("principal_user_id"))?,
        memory_generation_id: MemoryGenerationId::from_uuid(row.get("memory_generation_id")),
        kind: memory_item_kind_from_str(row.get::<&str, _>("kind"))?,
        authority: memory_authority_from_str(row.get::<&str, _>("authority"))?,
        status: memory_item_status_from_str(row.get::<&str, _>("status"))?,
        text: row.get("text"),
        structured: row.get("structured"),
        tags: row.get("tags"),
        evidence_turn_ids: uuids_to_turn_ids(row.get("evidence_turn_ids")),
        sensitivity: memory_sensitivity_from_str(row.get::<&str, _>("sensitivity"))?,
        valid_from: row
            .get::<Option<i64>, _>("valid_from")
            .map(TimestampMillis::new),
        valid_to: row
            .get::<Option<i64>, _>("valid_to")
            .map(TimestampMillis::new),
        supersedes_memory_id: row
            .get::<Option<Uuid>, _>("supersedes_memory_id")
            .map(MemoryItemId::from_uuid),
        created_at: TimestampMillis::new(row.get("created_at")),
        updated_at: TimestampMillis::new(row.get("updated_at")),
    })
}

fn task_state_from_row(row: sqlx_postgres::PgRow) -> LifeStorageResult<LifeTaskState> {
    Ok(LifeTaskState {
        task_state_id: TaskStateId::from_uuid(row.get("task_state_id")),
        principal_user_id: PrincipalUserId::new(row.get("principal_user_id"))?,
        memory_generation_id: MemoryGenerationId::from_uuid(row.get("memory_generation_id")),
        project_key: row.get("project_key"),
        current_goal: row.get("current_goal"),
        why: row.get("why"),
        current_state: row.get("current_state"),
        next_action: row.get("next_action"),
        open_loops: row.get("open_loops"),
        blockers: row.get("blockers"),
        status: task_state_status_from_str(row.get::<&str, _>("status"))?,
        last_turn_id: row
            .get::<Option<Uuid>, _>("last_turn_id")
            .map(TurnId::from_uuid),
        created_at: TimestampMillis::new(row.get("created_at")),
        updated_at: TimestampMillis::new(row.get("updated_at")),
    })
}

fn friction_pattern_from_row(row: sqlx_postgres::PgRow) -> LifeStorageResult<LifeFrictionPattern> {
    Ok(LifeFrictionPattern {
        pattern_id: FrictionPatternId::from_uuid(row.get("pattern_id")),
        principal_user_id: PrincipalUserId::new(row.get("principal_user_id"))?,
        memory_generation_id: MemoryGenerationId::from_uuid(row.get("memory_generation_id")),
        kind: friction_pattern_kind_from_str(row.get::<&str, _>("kind"))?,
        trigger_descriptor: row.get("trigger_descriptor"),
        preferred_response: row.get("preferred_response"),
        evidence_turn_ids: uuids_to_turn_ids(row.get("evidence_turn_ids")),
        authority: memory_authority_from_str(row.get::<&str, _>("authority"))?,
        status: support_state_status_from_str(row.get::<&str, _>("status"))?,
        created_at: TimestampMillis::new(row.get("created_at")),
        updated_at: TimestampMillis::new(row.get("updated_at")),
    })
}

fn support_protocol_from_row(row: sqlx_postgres::PgRow) -> LifeStorageResult<LifeSupportProtocol> {
    Ok(LifeSupportProtocol {
        protocol_id: SupportProtocolId::from_uuid(row.get("protocol_id")),
        principal_user_id: PrincipalUserId::new(row.get("principal_user_id"))?,
        memory_generation_id: MemoryGenerationId::from_uuid(row.get("memory_generation_id")),
        name: row.get("name"),
        trigger_descriptor: row.get("trigger_descriptor"),
        steps: row.get("steps"),
        priority: row.get("priority"),
        evidence_turn_ids: uuids_to_turn_ids(row.get("evidence_turn_ids")),
        authority: memory_authority_from_str(row.get::<&str, _>("authority"))?,
        status: support_state_status_from_str(row.get::<&str, _>("status"))?,
        created_at: TimestampMillis::new(row.get("created_at")),
        updated_at: TimestampMillis::new(row.get("updated_at")),
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicI64, Ordering};

    use serde_json::json;
    use sqlx_postgres::PgPoolOptions;

    use super::*;

    static USER_COUNTER: AtomicI64 = AtomicI64::new(1);

    #[tokio::test]
    async fn sqlx_life_storage_migrates_and_scopes_memory_by_active_generation() {
        let Some(storage) = sqlx_test_storage().await else {
            return;
        };
        let principal_user_id = unique_principal_user_id();
        let now = TimestampMillis::new(1_700_000_000_000);
        let principal = LifePrincipal {
            principal_user_id,
            profile_state: json!({"identity": {"display_name": "Alex"}}),
            operating_profile: json!({"task_support": {"role": "external"}}),
            settings: json!({}),
            schema_version: 1,
            created_at: now,
            updated_at: now,
        };
        must(
            storage.upsert_principal(&principal).await,
            "upsert principal",
        );

        let link = LifeIdentityLink {
            provider: LifeIdentityProvider::Web,
            provider_subject: must(ProviderSubject::new("web-user-1"), "provider subject"),
            principal_user_id,
            verified_at: Some(now),
            created_at: now,
            updated_at: now,
        };
        must(storage.link_identity(&link).await, "link identity");
        let resolved = must(
            storage
                .resolve_identity(LifeIdentityProvider::Web, &link.provider_subject)
                .await,
            "resolve identity",
        );
        assert_eq!(resolved, Some(principal_user_id));

        let gen1 = generation(principal_user_id, 1, MemoryGenerationStatus::Building, now);
        let gen2 = generation(
            principal_user_id,
            2,
            MemoryGenerationStatus::Building,
            TimestampMillis::new(now.get() + 1),
        );
        must(storage.insert_memory_generation(&gen1).await, "insert gen1");
        must(storage.insert_memory_generation(&gen2).await, "insert gen2");

        let active1 = must(
            storage
                .activate_memory_generation(
                    principal_user_id,
                    gen1.memory_generation_id,
                    TimestampMillis::new(now.get() + 2),
                    "initial",
                )
                .await,
            "activate gen1",
        );
        assert_eq!(
            active1.scope.memory_generation_id,
            gen1.memory_generation_id
        );

        must(
            storage
                .upsert_memory_item(&memory_item(
                    principal_user_id,
                    gen1.memory_generation_id,
                    "old active memory",
                    now,
                ))
                .await,
            "insert gen1 memory",
        );
        must(
            storage
                .upsert_memory_item(&memory_item(
                    principal_user_id,
                    gen2.memory_generation_id,
                    "new building memory",
                    now,
                ))
                .await,
            "insert gen2 memory",
        );
        must(
            storage
                .upsert_task_state(&task_state(
                    principal_user_id,
                    gen1.memory_generation_id,
                    "old goal",
                    now,
                ))
                .await,
            "insert gen1 task",
        );
        must(
            storage
                .upsert_task_state(&task_state(
                    principal_user_id,
                    gen2.memory_generation_id,
                    "new goal",
                    now,
                ))
                .await,
            "insert gen2 task",
        );
        must(
            storage
                .upsert_support_protocol(&support_protocol(
                    principal_user_id,
                    gen1.memory_generation_id,
                    "old protocol",
                    1,
                    now,
                ))
                .await,
            "insert gen1 protocol",
        );
        must(
            storage
                .upsert_support_protocol(&support_protocol(
                    principal_user_id,
                    gen2.memory_generation_id,
                    "new protocol",
                    2,
                    now,
                ))
                .await,
            "insert gen2 protocol",
        );
        must(
            storage
                .upsert_friction_pattern(&friction_pattern(
                    principal_user_id,
                    gen1.memory_generation_id,
                    "old overload",
                    now,
                ))
                .await,
            "insert gen1 friction pattern",
        );
        must(
            storage
                .upsert_friction_pattern(&friction_pattern(
                    principal_user_id,
                    gen2.memory_generation_id,
                    "new overload",
                    now,
                ))
                .await,
            "insert gen2 friction pattern",
        );

        let active = must(
            LifeStorageRepository::active_generation(&storage, principal_user_id).await,
            "load active generation",
        );
        assert_eq!(active, Some(active1));
        let active = match active {
            Some(active) => active,
            None => panic!("active generation should exist"),
        };

        let memories = must(
            storage.active_memory_items(active.scope).await,
            "load memories",
        );
        assert_eq!(memories.len(), 1);
        assert_eq!(memories[0].text, "old active memory");
        let tasks = must(storage.active_task_states(active.scope).await, "load tasks");
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].current_goal, "old goal");
        let protocols = must(
            storage.active_support_protocols(active.scope).await,
            "load protocols",
        );
        assert_eq!(protocols.len(), 1);
        assert_eq!(protocols[0].name, "old protocol");
        let patterns = must(
            storage.active_friction_patterns(active.scope).await,
            "load friction patterns",
        );
        assert_eq!(patterns.len(), 1);
        assert_eq!(patterns[0].trigger_descriptor, "old overload");

        let active2 = must(
            storage
                .activate_memory_generation(
                    principal_user_id,
                    gen2.memory_generation_id,
                    TimestampMillis::new(now.get() + 3),
                    "activate rebuilt generation",
                )
                .await,
            "activate gen2",
        );
        let memories = must(
            storage.active_memory_items(active2.scope).await,
            "load gen2 memories",
        );
        assert_eq!(memories.len(), 1);
        assert_eq!(memories[0].text, "new building memory");
        let tasks = must(
            storage.active_task_states(active2.scope).await,
            "load gen2 tasks",
        );
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].current_goal, "new goal");
        let protocols = must(
            storage.active_support_protocols(active2.scope).await,
            "load gen2 protocols",
        );
        assert_eq!(protocols.len(), 1);
        assert_eq!(protocols[0].name, "new protocol");
        let patterns = must(
            storage.active_friction_patterns(active2.scope).await,
            "load gen2 friction patterns",
        );
        assert_eq!(patterns.len(), 1);
        assert_eq!(patterns[0].trigger_descriptor, "new overload");

        let rolled_back = must(
            storage
                .activate_memory_generation(
                    principal_user_id,
                    gen1.memory_generation_id,
                    TimestampMillis::new(now.get() + 4),
                    "rollback",
                )
                .await,
            "rollback to gen1",
        );
        let memories = must(
            storage.active_memory_items(rolled_back.scope).await,
            "load rolled back memories",
        );
        assert_eq!(memories.len(), 1);
        assert_eq!(memories[0].text, "old active memory");
        let patterns = must(
            storage.active_friction_patterns(rolled_back.scope).await,
            "load rolled back friction patterns",
        );
        assert_eq!(patterns.len(), 1);
        assert_eq!(patterns[0].trigger_descriptor, "old overload");
    }

    async fn sqlx_test_storage() -> Option<SqlxLifeStorage> {
        let Ok(database_url) = std::env::var("OXIDE_DATABASE_TEST_URL") else {
            eprintln!("OXIDE_DATABASE_TEST_URL not set; skipping SQLx/Postgres life storage test");
            return None;
        };
        let pool = match PgPoolOptions::new()
            .max_connections(1)
            .connect(&database_url)
            .await
        {
            Ok(pool) => pool,
            Err(error) => panic!("SQLx life test pool should connect: {error}"),
        };
        let storage = SqlxLifeStorage::new(pool);
        let migrations_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("migrations");
        must(
            storage.run_migrations_from_path(migrations_dir).await,
            "run migrations",
        );
        Some(storage)
    }

    fn unique_principal_user_id() -> PrincipalUserId {
        let value = 2_000_000_000_000 + USER_COUNTER.fetch_add(1, Ordering::Relaxed);
        must(PrincipalUserId::new(value), "principal user id")
    }

    fn generation(
        principal_user_id: PrincipalUserId,
        number: i64,
        status: MemoryGenerationStatus,
        now: TimestampMillis,
    ) -> LifeMemoryGeneration {
        LifeMemoryGeneration {
            memory_generation_id: MemoryGenerationId::new_v4(),
            principal_user_id,
            generation_number: number,
            status,
            source_generation_id: None,
            build_reason: format!("test generation {number}"),
            build_policy: json!({"test": true}),
            source_scope: json!({}),
            comparison_report: json!({}),
            activated_at: None,
            created_at: now,
            updated_at: now,
        }
    }

    fn memory_item(
        principal_user_id: PrincipalUserId,
        memory_generation_id: MemoryGenerationId,
        text: &str,
        now: TimestampMillis,
    ) -> LifeMemoryItem {
        LifeMemoryItem {
            memory_id: MemoryItemId::new_v4(),
            principal_user_id,
            memory_generation_id,
            kind: MemoryItemKind::ProjectPrinciple,
            authority: MemoryAuthority::UserAsserted,
            status: MemoryItemStatus::Active,
            text: text.to_owned(),
            structured: json!({}),
            tags: vec!["test".to_owned()],
            evidence_turn_ids: Vec::new(),
            sensitivity: MemorySensitivity::Clean,
            valid_from: None,
            valid_to: None,
            supersedes_memory_id: None,
            created_at: now,
            updated_at: now,
        }
    }

    fn task_state(
        principal_user_id: PrincipalUserId,
        memory_generation_id: MemoryGenerationId,
        goal: &str,
        now: TimestampMillis,
    ) -> LifeTaskState {
        LifeTaskState {
            task_state_id: TaskStateId::new_v4(),
            principal_user_id,
            memory_generation_id,
            project_key: "oxide-agent".to_owned(),
            current_goal: goal.to_owned(),
            why: Some("test".to_owned()),
            current_state: json!([goal]),
            next_action: Some("continue".to_owned()),
            open_loops: json!([]),
            blockers: json!([]),
            status: TaskStateStatus::Active,
            last_turn_id: None,
            created_at: now,
            updated_at: now,
        }
    }

    fn support_protocol(
        principal_user_id: PrincipalUserId,
        memory_generation_id: MemoryGenerationId,
        name: &str,
        priority: i32,
        now: TimestampMillis,
    ) -> LifeSupportProtocol {
        LifeSupportProtocol {
            protocol_id: SupportProtocolId::new_v4(),
            principal_user_id,
            memory_generation_id,
            name: name.to_owned(),
            trigger_descriptor: "test trigger".to_owned(),
            steps: json!(["step"]),
            priority,
            evidence_turn_ids: Vec::new(),
            authority: MemoryAuthority::UserConfirmed,
            status: SupportStateStatus::Active,
            created_at: now,
            updated_at: now,
        }
    }

    fn friction_pattern(
        principal_user_id: PrincipalUserId,
        memory_generation_id: MemoryGenerationId,
        trigger_descriptor: &str,
        now: TimestampMillis,
    ) -> LifeFrictionPattern {
        LifeFrictionPattern {
            pattern_id: FrictionPatternId::new_v4(),
            principal_user_id,
            memory_generation_id,
            kind: FrictionPatternKind::OverloadTrigger,
            trigger_descriptor: trigger_descriptor.to_owned(),
            preferred_response: json!({"response": "narrow"}),
            evidence_turn_ids: Vec::new(),
            authority: MemoryAuthority::UserConfirmed,
            status: SupportStateStatus::Active,
            created_at: now,
            updated_at: now,
        }
    }

    fn must<T, E: std::fmt::Display>(result: Result<T, E>, context: &str) -> T {
        match result {
            Ok(value) => value,
            Err(error) => panic!("{context}: {error}"),
        }
    }
}
