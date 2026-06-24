//! SQLx/Postgres implementation for life-mode source-of-truth storage.

use std::path::Path;

use async_trait::async_trait;
use sqlx_core::migrate::Migrator;
use sqlx_core::query::query;
use sqlx_core::row::Row;
use sqlx_postgres::{PgPool, Postgres};
use uuid::Uuid;

use crate::domain::{
    ActiveMemoryGeneration, ContextOverrideId, FrictionPatternId, FrictionPatternKind, InputId,
    LifeContextOverride, LifeEvent, LifeFrictionPattern, LifeIdentityLink, LifeIdentityProvider,
    LifeInput, LifeInputStatus, LifeMemoryGeneration, LifeMemoryItem, LifePrincipal, LifeRun,
    LifeRunStatus, LifeSourceTransport, LifeSupportProtocol, LifeTaskState, LifeTurn, LifeTurnRole,
    MemoryAuthority, MemoryGenerationId, MemoryGenerationStatus, MemoryItemId, MemoryItemKind,
    MemoryItemStatus, MemoryScope, MemorySensitivity, PrincipalUserId, ProviderSubject,
    RedactionState, RunId, SupportProtocolId, SupportStateStatus, TaskStateId, TaskStateStatus,
    TimestampMillis, TurnId,
};
use crate::storage::{
    ClaimedLifeInputRun, LifeStorageError, LifeStorageRepository, LifeStorageResult,
};

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

    async fn advisory_xact_lock_in_tx(
        tx: &mut sqlx_core::transaction::Transaction<'_, Postgres>,
        principal_user_id: PrincipalUserId,
    ) -> LifeStorageResult<()> {
        query::<Postgres>("SELECT pg_advisory_xact_lock($1)")
            .bind(life_principal_lock_key(principal_user_id))
            .execute(&mut **tx)
            .await
            .map_err(db_error)?;
        Ok(())
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

    async fn principal(
        &self,
        principal_user_id: PrincipalUserId,
    ) -> LifeStorageResult<Option<LifePrincipal>> {
        let row = query::<Postgres>(
            r#"
            SELECT *
            FROM life_principals
            WHERE principal_user_id = $1
            "#,
        )
        .bind(principal_user_id.get())
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?;

        row.map(principal_from_row).transpose()
    }

    async fn link_identity(&self, link: &LifeIdentityLink) -> LifeStorageResult<()> {
        let row = query::<Postgres>(
            r#"
            INSERT INTO life_identity_links (
                provider, provider_subject, principal_user_id, verified_at, created_at, updated_at
            )
            VALUES ($1, $2, $3, $4, $5, $6)
            ON CONFLICT (provider, provider_subject) DO UPDATE
            SET verified_at = EXCLUDED.verified_at,
                updated_at = EXCLUDED.updated_at
            WHERE life_identity_links.principal_user_id = EXCLUDED.principal_user_id
            RETURNING principal_user_id
            "#,
        )
        .bind(link.provider.as_str())
        .bind(link.provider_subject.as_str())
        .bind(link.principal_user_id.get())
        .bind(link.verified_at.map(TimestampMillis::get))
        .bind(link.created_at.get())
        .bind(link.updated_at.get())
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?;

        if row.is_none() {
            return Err(LifeStorageError::IdentityLinkConflict {
                provider: link.provider,
                provider_subject: link.provider_subject.clone(),
            });
        }
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

    async fn next_memory_generation_number(
        &self,
        principal_user_id: PrincipalUserId,
    ) -> LifeStorageResult<i64> {
        let row = query::<Postgres>(
            r#"
            SELECT COALESCE(MAX(generation_number), 0) + 1 AS next_generation_number
            FROM life_memory_generations
            WHERE principal_user_id = $1
            "#,
        )
        .bind(principal_user_id.get())
        .fetch_one(&self.pool)
        .await
        .map_err(db_error)?;

        Ok(row.get::<i64, _>("next_generation_number"))
    }

    async fn append_turn(&self, turn: &LifeTurn) -> LifeStorageResult<()> {
        query::<Postgres>(
            r#"
            INSERT INTO life_turns (
                turn_id, principal_user_id, run_id, role, source_transport,
                source_ref, content, attachments, transport_metadata,
                redaction_state, created_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
            "#,
        )
        .bind(turn.turn_id.as_uuid())
        .bind(turn.principal_user_id.get())
        .bind(turn.run_id.map(crate::domain::RunId::as_uuid))
        .bind(turn_role_as_str(turn.role))
        .bind(source_transport_as_str(turn.source_transport))
        .bind(&turn.source_ref)
        .bind(&turn.content)
        .bind(&turn.attachments)
        .bind(&turn.transport_metadata)
        .bind(redaction_state_as_str(turn.redaction_state))
        .bind(turn.created_at.get())
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(())
    }

    async fn enqueue_input(&self, input: &LifeInput) -> LifeStorageResult<()> {
        query::<Postgres>(
            r#"
            INSERT INTO life_inputs (
                input_id, principal_user_id, turn_id, status, claimed_by,
                claimed_at, created_at, updated_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            "#,
        )
        .bind(input.input_id.as_uuid())
        .bind(input.principal_user_id.get())
        .bind(input.turn_id.as_uuid())
        .bind(input_status_as_str(input.status))
        .bind(&input.claimed_by)
        .bind(input.claimed_at.map(TimestampMillis::get))
        .bind(input.created_at.get())
        .bind(input.updated_at.get())
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(())
    }

    async fn save_life_memory_checkpoint(
        &self,
        principal_user_id: PrincipalUserId,
        context_key: &str,
        flow_id: &str,
        memory: &serde_json::Value,
        schema_version: i32,
        now: TimestampMillis,
    ) -> LifeStorageResult<()> {
        query::<Postgres>(
            r#"
            INSERT INTO agent_memory_snapshots (
                user_id, context_key, flow_id, memory, schema_version, created_at, updated_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $6)
            ON CONFLICT (user_id, context_key, flow_id) DO UPDATE
            SET memory = EXCLUDED.memory,
                schema_version = EXCLUDED.schema_version,
                updated_at = EXCLUDED.updated_at
            "#,
        )
        .bind(principal_user_id.get())
        .bind(context_key)
        .bind(flow_id)
        .bind(memory)
        .bind(schema_version)
        .bind(now.get())
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(())
    }

    async fn claim_input_and_start_run(
        &self,
        principal_user_id: PrincipalUserId,
        input_id: InputId,
        run_id: RunId,
        worker_id: &str,
        now: TimestampMillis,
    ) -> LifeStorageResult<Option<ClaimedLifeInputRun>> {
        let mut tx = self.pool.begin().await.map_err(db_error)?;
        Self::advisory_xact_lock_in_tx(&mut tx, principal_user_id).await?;

        let running_row = query::<Postgres>(
            r#"
            SELECT run_id
            FROM life_runs
            WHERE principal_user_id = $1 AND status = 'running'
            LIMIT 1
            "#,
        )
        .bind(principal_user_id.get())
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_error)?;
        if running_row.is_some() {
            tx.commit().await.map_err(db_error)?;
            return Ok(None);
        }

        let input_row = query::<Postgres>(
            r#"
            UPDATE life_inputs
            SET status = 'claimed', claimed_by = $3, claimed_at = $4, updated_at = $4
            WHERE input_id = $1 AND principal_user_id = $2 AND status = 'queued'
            RETURNING *
            "#,
        )
        .bind(input_id.as_uuid())
        .bind(principal_user_id.get())
        .bind(worker_id)
        .bind(now.get())
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_error)?;
        let Some(input_row) = input_row else {
            tx.commit().await.map_err(db_error)?;
            return Ok(None);
        };

        let active_row = query::<Postgres>(
            r#"
            SELECT memory_generation_id
            FROM life_active_memory_generations
            WHERE principal_user_id = $1
            "#,
        )
        .bind(principal_user_id.get())
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_error)?;
        let Some(active_row) = active_row else {
            return Err(LifeStorageError::MissingActiveGeneration { principal_user_id });
        };
        let memory_generation_id =
            MemoryGenerationId::from_uuid(active_row.get::<Uuid, _>("memory_generation_id"));

        query::<Postgres>(
            r#"
            INSERT INTO life_runs (
                run_id, principal_user_id, memory_generation_id, status,
                started_at, finished_at, last_checkpoint_at, error_text, created_at, updated_at
            )
            VALUES ($1, $2, $3, 'running', $4, NULL, NULL, NULL, $4, $4)
            "#,
        )
        .bind(run_id.as_uuid())
        .bind(principal_user_id.get())
        .bind(memory_generation_id.as_uuid())
        .bind(now.get())
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;

        tx.commit().await.map_err(db_error)?;

        Ok(Some(ClaimedLifeInputRun {
            input: input_from_row(input_row)?,
            run: LifeRun {
                run_id,
                principal_user_id,
                memory_generation_id,
                status: LifeRunStatus::Running,
                started_at: Some(now),
                finished_at: None,
                last_checkpoint_at: None,
                error_text: None,
                created_at: now,
                updated_at: now,
            },
        }))
    }

    async fn mark_input_consumed(
        &self,
        input_id: InputId,
        now: TimestampMillis,
    ) -> LifeStorageResult<()> {
        query::<Postgres>(
            r#"
            UPDATE life_inputs
            SET status = 'consumed', updated_at = $2
            WHERE input_id = $1 AND status IN ('claimed', 'queued')
            "#,
        )
        .bind(input_id.as_uuid())
        .bind(now.get())
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(())
    }

    async fn drain_queued_inputs_for_run(
        &self,
        principal_user_id: PrincipalUserId,
        worker_id: &str,
        now: TimestampMillis,
    ) -> LifeStorageResult<Vec<LifeInput>> {
        let mut tx = self.pool.begin().await.map_err(db_error)?;
        Self::advisory_xact_lock_in_tx(&mut tx, principal_user_id).await?;
        let rows = query::<Postgres>(
            r#"
            UPDATE life_inputs
            SET status = 'consumed', claimed_by = $2, claimed_at = $3, updated_at = $3
            WHERE principal_user_id = $1 AND status = 'queued'
            RETURNING *
            "#,
        )
        .bind(principal_user_id.get())
        .bind(worker_id)
        .bind(now.get())
        .fetch_all(&mut *tx)
        .await
        .map_err(db_error)?;
        tx.commit().await.map_err(db_error)?;
        rows.into_iter().map(input_from_row).collect()
    }

    async fn append_event(&self, event: &LifeEvent) -> LifeStorageResult<()> {
        query::<Postgres>(
            r#"
            INSERT INTO life_events (event_id, run_id, seq, kind, payload, created_at)
            VALUES ($1, $2, $3, $4, $5, $6)
            "#,
        )
        .bind(event.event_id.as_uuid())
        .bind(event.run_id.as_uuid())
        .bind(event.seq)
        .bind(&event.kind)
        .bind(&event.payload)
        .bind(event.created_at.get())
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(())
    }

    async fn next_event_seq(&self, run_id: RunId) -> LifeStorageResult<i64> {
        let row = query::<Postgres>(
            r#"
            SELECT COALESCE(MAX(seq), -1) + 1 AS next_seq
            FROM life_events
            WHERE run_id = $1
            "#,
        )
        .bind(run_id.as_uuid())
        .fetch_one(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(row.get("next_seq"))
    }

    async fn complete_run(
        &self,
        run_id: RunId,
        finished_at: TimestampMillis,
        last_checkpoint_at: TimestampMillis,
    ) -> LifeStorageResult<()> {
        query::<Postgres>(
            r#"
            UPDATE life_runs
            SET status = 'completed', finished_at = $2, last_checkpoint_at = $3, updated_at = $2
            WHERE run_id = $1 AND status = 'running'
            "#,
        )
        .bind(run_id.as_uuid())
        .bind(finished_at.get())
        .bind(last_checkpoint_at.get())
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(())
    }

    async fn fail_run(
        &self,
        run_id: RunId,
        finished_at: TimestampMillis,
        error_text: &str,
    ) -> LifeStorageResult<()> {
        query::<Postgres>(
            r#"
            UPDATE life_runs
            SET status = 'failed', finished_at = $2, error_text = $3, updated_at = $2
            WHERE run_id = $1 AND status = 'running'
            "#,
        )
        .bind(run_id.as_uuid())
        .bind(finished_at.get())
        .bind(error_text)
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(())
    }

    async fn active_context_overrides(
        &self,
        principal_user_id: PrincipalUserId,
        now: TimestampMillis,
    ) -> LifeStorageResult<Vec<LifeContextOverride>> {
        let rows = query::<Postgres>(
            r#"
            SELECT *
            FROM life_context_overrides
            WHERE principal_user_id = $1
              AND (expires_at IS NULL OR expires_at > $2)
            ORDER BY updated_at DESC, key ASC
            "#,
        )
        .bind(principal_user_id.get())
        .bind(now.get())
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;

        rows.into_iter().map(context_override_from_row).collect()
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

fn turn_role_as_str(role: LifeTurnRole) -> &'static str {
    match role {
        LifeTurnRole::User => "user",
        LifeTurnRole::Assistant => "assistant",
        LifeTurnRole::System => "system",
        LifeTurnRole::Tool => "tool",
    }
}

fn source_transport_as_str(source_transport: LifeSourceTransport) -> &'static str {
    match source_transport {
        LifeSourceTransport::Web => "web",
        LifeSourceTransport::Telegram => "telegram",
        LifeSourceTransport::Internal => "internal",
    }
}

fn redaction_state_as_str(redaction_state: RedactionState) -> &'static str {
    match redaction_state {
        RedactionState::Clean => "clean",
        RedactionState::Redacted => "redacted",
        RedactionState::SecretBlocked => "secret_blocked",
    }
}

fn input_status_as_str(status: LifeInputStatus) -> &'static str {
    match status {
        LifeInputStatus::Queued => "queued",
        LifeInputStatus::Claimed => "claimed",
        LifeInputStatus::Consumed => "consumed",
        LifeInputStatus::Dead => "dead",
    }
}

fn input_status_from_str(value: &str) -> LifeStorageResult<LifeInputStatus> {
    match value {
        "queued" => Ok(LifeInputStatus::Queued),
        "claimed" => Ok(LifeInputStatus::Claimed),
        "consumed" => Ok(LifeInputStatus::Consumed),
        "dead" => Ok(LifeInputStatus::Dead),
        other => unknown_enum("life_input_status", other),
    }
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

fn life_principal_lock_key(principal_user_id: PrincipalUserId) -> i64 {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

    let mut hash = FNV_OFFSET;
    for byte in format!("life:{}", principal_user_id.get()).as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    i64::from_be_bytes(hash.to_be_bytes())
}

fn principal_from_row(row: sqlx_postgres::PgRow) -> LifeStorageResult<LifePrincipal> {
    Ok(LifePrincipal {
        principal_user_id: PrincipalUserId::new(row.get("principal_user_id"))?,
        profile_state: row.get("profile_state"),
        operating_profile: row.get("operating_profile"),
        settings: row.get("settings"),
        schema_version: row.get("schema_version"),
        created_at: TimestampMillis::new(row.get("created_at")),
        updated_at: TimestampMillis::new(row.get("updated_at")),
    })
}

fn context_override_from_row(row: sqlx_postgres::PgRow) -> LifeStorageResult<LifeContextOverride> {
    Ok(LifeContextOverride {
        override_id: ContextOverrideId::from_uuid(row.get("override_id")),
        principal_user_id: PrincipalUserId::new(row.get("principal_user_id"))?,
        key: row.get("key"),
        value: row.get("value"),
        reason: row.get("reason"),
        expires_at: row
            .get::<Option<i64>, _>("expires_at")
            .map(TimestampMillis::new),
        created_at: TimestampMillis::new(row.get("created_at")),
        updated_at: TimestampMillis::new(row.get("updated_at")),
    })
}

fn input_from_row(row: sqlx_postgres::PgRow) -> LifeStorageResult<LifeInput> {
    Ok(LifeInput {
        input_id: InputId::from_uuid(row.get("input_id")),
        principal_user_id: PrincipalUserId::new(row.get("principal_user_id"))?,
        turn_id: TurnId::from_uuid(row.get("turn_id")),
        status: input_status_from_str(row.get::<&str, _>("status"))?,
        claimed_by: row.get("claimed_by"),
        claimed_at: row
            .get::<Option<i64>, _>("claimed_at")
            .map(TimestampMillis::new),
        created_at: TimestampMillis::new(row.get("created_at")),
        updated_at: TimestampMillis::new(row.get("updated_at")),
    })
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
    use std::time::{SystemTime, UNIX_EPOCH};

    use async_trait::async_trait;
    use serde_json::json;
    use sqlx_postgres::PgPoolOptions;

    use crate::domain::EventId;
    use crate::gateway::{
        LifeClock, LifeGateway, LifeGatewayResult, LifeInputSubmission, LifePrincipalAllocator,
    };

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
        let loaded_principal = must(storage.principal(principal_user_id).await, "load principal");
        assert_eq!(loaded_principal, Some(principal.clone()));

        let active_override_id = ContextOverrideId::new_v4();
        must(
            query::<Postgres>(
                r#"
                INSERT INTO life_context_overrides (
                    override_id, principal_user_id, key, value, reason,
                    expires_at, created_at, updated_at
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                "#,
            )
            .bind(active_override_id.as_uuid())
            .bind(principal_user_id.get())
            .bind("answer_verbosity")
            .bind(json!("detailed"))
            .bind("today only")
            .bind(Some(now.get() + 1_000))
            .bind(now.get())
            .bind(now.get())
            .execute(storage.pool())
            .await
            .map_err(db_error),
            "insert active override",
        );
        must(
            query::<Postgres>(
                r#"
                INSERT INTO life_context_overrides (
                    override_id, principal_user_id, key, value, reason,
                    expires_at, created_at, updated_at
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                "#,
            )
            .bind(ContextOverrideId::new_v4().as_uuid())
            .bind(principal_user_id.get())
            .bind("expired_override")
            .bind(json!(true))
            .bind(Option::<String>::None)
            .bind(Some(now.get() - 1))
            .bind(now.get())
            .bind(now.get())
            .execute(storage.pool())
            .await
            .map_err(db_error),
            "insert expired override",
        );
        let active_overrides = must(
            storage
                .active_context_overrides(principal_user_id, now)
                .await,
            "load active overrides",
        );
        assert_eq!(active_overrides.len(), 1);
        assert_eq!(active_overrides[0].override_id, active_override_id);

        let link = LifeIdentityLink {
            provider: LifeIdentityProvider::Web,
            provider_subject: must(
                ProviderSubject::new(format!("web-user-{}", principal_user_id.get())),
                "provider subject",
            ),
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

        let conflicting_principal_user_id = unique_principal_user_id();
        let conflicting_principal = LifePrincipal {
            principal_user_id: conflicting_principal_user_id,
            profile_state: json!({}),
            operating_profile: json!({}),
            settings: json!({}),
            schema_version: 1,
            created_at: now,
            updated_at: now,
        };
        must(
            storage.upsert_principal(&conflicting_principal).await,
            "upsert conflicting principal",
        );
        let conflicting_link = LifeIdentityLink {
            provider: LifeIdentityProvider::Web,
            provider_subject: link.provider_subject.clone(),
            principal_user_id: conflicting_principal_user_id,
            verified_at: Some(now),
            created_at: now,
            updated_at: now,
        };
        let conflict = storage
            .link_identity(&conflicting_link)
            .await
            .expect_err("duplicate provider subject must not relink principal");
        assert!(matches!(
            conflict,
            LifeStorageError::IdentityLinkConflict { .. }
        ));

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

    #[tokio::test]
    async fn sqlx_life_gateway_submit_persists_turn_metadata_and_input() {
        let Some(storage) = sqlx_test_storage().await else {
            return;
        };
        let principal_user_id = unique_principal_user_id();
        let now = TimestampMillis::new(1_700_000_100_000);
        let gateway = LifeGateway::with_clock(
            storage.clone(),
            FixedAllocator(principal_user_id),
            FixedClock(now),
        );
        let submission = LifeInputSubmission {
            provider: LifeIdentityProvider::Telegram,
            provider_subject: must(
                ProviderSubject::new(format!("telegram-user-gateway-{}", principal_user_id.get())),
                "subject",
            ),
            content: "restore context".to_owned(),
            attachments: json!([{"kind": "voice", "ref": "file-1"}]),
            metadata: json!({"chat_id": 42, "message_id": 7}),
        };

        let result = must(
            gateway.submit_life_input(submission).await,
            "submit life input",
        );

        assert_eq!(result.principal_user_id, principal_user_id);
        assert_eq!(result.memory_scope.principal_user_id, principal_user_id);

        let turn_row = must(
            query::<Postgres>(
                r#"
                SELECT content, source_transport, attachments, transport_metadata
                FROM life_turns
                WHERE turn_id = $1 AND principal_user_id = $2
                "#,
            )
            .bind(result.turn_id.as_uuid())
            .bind(principal_user_id.get())
            .fetch_one(storage.pool())
            .await,
            "load submitted turn",
        );
        assert_eq!(turn_row.get::<String, _>("content"), "restore context");
        assert_eq!(turn_row.get::<String, _>("source_transport"), "telegram");
        assert_eq!(
            turn_row.get::<serde_json::Value, _>("attachments"),
            json!([{"kind": "voice", "ref": "file-1"}])
        );
        assert_eq!(
            turn_row.get::<serde_json::Value, _>("transport_metadata"),
            json!({"chat_id": 42, "message_id": 7})
        );

        let input_status = must(
            query::<Postgres>(
                r#"
                SELECT status
                FROM life_inputs
                WHERE input_id = $1 AND turn_id = $2 AND principal_user_id = $3
                "#,
            )
            .bind(result.input_id.as_uuid())
            .bind(result.turn_id.as_uuid())
            .bind(principal_user_id.get())
            .fetch_one(storage.pool())
            .await,
            "load queued input",
        )
        .get::<String, _>("status");
        assert_eq!(input_status, "queued");

        let active = must(
            storage.active_generation(principal_user_id).await,
            "load active generation",
        );
        assert_eq!(
            active.map(|generation| generation.scope),
            Some(result.memory_scope)
        );
    }

    #[tokio::test]
    async fn sqlx_life_worker_claim_start_complete_and_drain_are_db_backed() {
        let Some(storage) = sqlx_test_storage().await else {
            return;
        };
        let principal_user_id = unique_principal_user_id();
        let now = TimestampMillis::new(1_700_000_010_000);
        let principal = LifePrincipal {
            principal_user_id,
            profile_state: json!({}),
            operating_profile: json!({}),
            settings: json!({}),
            schema_version: 1,
            created_at: now,
            updated_at: now,
        };
        must(
            storage.upsert_principal(&principal).await,
            "upsert principal",
        );
        let generation = generation(principal_user_id, 1, MemoryGenerationStatus::Building, now);
        must(
            storage.insert_memory_generation(&generation).await,
            "insert generation",
        );
        let active = must(
            storage
                .activate_memory_generation(
                    principal_user_id,
                    generation.memory_generation_id,
                    now,
                    "worker test",
                )
                .await,
            "activate generation",
        );

        let first_turn = user_turn(principal_user_id, "first", now);
        must(storage.append_turn(&first_turn).await, "append first turn");
        let first_input = queued_input(principal_user_id, first_turn.turn_id, now);
        must(storage.enqueue_input(&first_input).await, "enqueue first");

        let run_id = RunId::new_v4();
        let claimed = must(
            storage
                .claim_input_and_start_run(
                    principal_user_id,
                    first_input.input_id,
                    run_id,
                    "worker-sqlx",
                    TimestampMillis::new(now.get() + 1),
                )
                .await,
            "claim input and start run",
        )
        .expect("queued input should be claimed");
        assert_eq!(claimed.input.status, LifeInputStatus::Claimed);
        assert_eq!(claimed.run.run_id, run_id);
        assert_eq!(
            claimed.run.memory_generation_id,
            active.scope.memory_generation_id
        );

        let second_claim = must(
            storage
                .claim_input_and_start_run(
                    principal_user_id,
                    first_input.input_id,
                    RunId::new_v4(),
                    "worker-sqlx",
                    TimestampMillis::new(now.get() + 2),
                )
                .await,
            "claim while running",
        );
        assert!(second_claim.is_none(), "running run blocks duplicate start");

        let seq0 = must(storage.next_event_seq(run_id).await, "next event seq 0");
        assert_eq!(seq0, 0);
        must(
            storage
                .append_event(&LifeEvent {
                    event_id: EventId::new_v4(),
                    run_id,
                    seq: seq0,
                    kind: "run_started".to_owned(),
                    payload: json!({}),
                    created_at: now,
                })
                .await,
            "append event",
        );
        assert_eq!(
            must(storage.next_event_seq(run_id).await, "next event seq 1"),
            1
        );

        let follow_up_turn = user_turn(
            principal_user_id,
            "follow-up",
            TimestampMillis::new(now.get() + 3),
        );
        must(
            storage.append_turn(&follow_up_turn).await,
            "append follow-up turn",
        );
        let follow_up = queued_input(
            principal_user_id,
            follow_up_turn.turn_id,
            TimestampMillis::new(now.get() + 3),
        );
        must(storage.enqueue_input(&follow_up).await, "enqueue follow-up");
        let drained = must(
            storage
                .drain_queued_inputs_for_run(
                    principal_user_id,
                    "worker-sqlx",
                    TimestampMillis::new(now.get() + 4),
                )
                .await,
            "drain queued inputs",
        );
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].input_id, follow_up.input_id);
        assert_eq!(drained[0].status, LifeInputStatus::Consumed);

        must(
            storage
                .mark_input_consumed(first_input.input_id, TimestampMillis::new(now.get() + 5))
                .await,
            "mark first input consumed",
        );
        must(
            storage
                .complete_run(
                    run_id,
                    TimestampMillis::new(now.get() + 6),
                    TimestampMillis::new(now.get() + 5),
                )
                .await,
            "complete run",
        );

        let run_status = must(
            query::<Postgres>("SELECT status, last_checkpoint_at FROM life_runs WHERE run_id = $1")
                .bind(run_id.as_uuid())
                .fetch_one(storage.pool())
                .await,
            "load completed run",
        );
        assert_eq!(run_status.get::<String, _>("status"), "completed");
        assert_eq!(
            run_status.get::<i64, _>("last_checkpoint_at"),
            now.get() + 5
        );

        must(
            storage
                .save_life_memory_checkpoint(
                    principal_user_id,
                    crate::worker::LIFE_CONTEXT_KEY,
                    crate::worker::LIFE_FLOW_ID,
                    &json!({"checkpoint": "final"}),
                    1,
                    TimestampMillis::new(now.get() + 5),
                )
                .await,
            "save final checkpoint",
        );
        let checkpoint = must(
            query::<Postgres>(
                r#"
                SELECT memory, schema_version, updated_at
                FROM agent_memory_snapshots
                WHERE user_id = $1 AND context_key = $2 AND flow_id = $3
                "#,
            )
            .bind(principal_user_id.get())
            .bind(crate::worker::LIFE_CONTEXT_KEY)
            .bind(crate::worker::LIFE_FLOW_ID)
            .fetch_one(storage.pool())
            .await,
            "load final checkpoint",
        );
        assert_eq!(
            checkpoint.get::<serde_json::Value, _>("memory"),
            json!({"checkpoint": "final"})
        );
        assert_eq!(checkpoint.get::<i32, _>("schema_version"), 1);
        assert_eq!(checkpoint.get::<i64, _>("updated_at"), now.get() + 5);
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
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("test clock after unix epoch")
            .as_nanos();
        let time_component =
            i64::try_from(nanos % 1_000_000_000_000).expect("time component should fit into i64");
        let value =
            2_000_000_000_000 + time_component + USER_COUNTER.fetch_add(1, Ordering::Relaxed);
        must(PrincipalUserId::new(value), "principal user id")
    }

    struct FixedAllocator(PrincipalUserId);

    #[async_trait]
    impl LifePrincipalAllocator for FixedAllocator {
        async fn allocate_principal_user_id(&self) -> LifeGatewayResult<PrincipalUserId> {
            Ok(self.0)
        }
    }

    #[derive(Debug, Copy, Clone)]
    struct FixedClock(TimestampMillis);

    impl LifeClock for FixedClock {
        fn now(&self) -> LifeGatewayResult<TimestampMillis> {
            Ok(self.0)
        }
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

    fn user_turn(
        principal_user_id: PrincipalUserId,
        content: &str,
        now: TimestampMillis,
    ) -> LifeTurn {
        LifeTurn {
            turn_id: TurnId::new_v4(),
            principal_user_id,
            run_id: None,
            role: LifeTurnRole::User,
            source_transport: LifeSourceTransport::Internal,
            source_ref: None,
            content: content.to_owned(),
            attachments: json!([]),
            transport_metadata: json!({}),
            redaction_state: RedactionState::Clean,
            created_at: now,
        }
    }

    fn queued_input(
        principal_user_id: PrincipalUserId,
        turn_id: TurnId,
        now: TimestampMillis,
    ) -> LifeInput {
        LifeInput {
            input_id: InputId::new_v4(),
            principal_user_id,
            turn_id,
            status: LifeInputStatus::Queued,
            claimed_by: None,
            claimed_at: None,
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
