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
    LifeContextOverride, LifeEngramOutboxRow, LifeEngramOutboxStatus, LifeEvent,
    LifeFrictionPattern, LifeIdentityLink, LifeIdentityProvider, LifeInput, LifeInputStatus,
    LifeLinkToken, LifeMemoryGeneration, LifeMemoryItem, LifePrincipal, LifeRun, LifeRunStatus,
    LifeSourceTransport, LifeSupportProtocol, LifeTaskState, LifeTurn, LifeTurnRole,
    MemoryAuthority, MemoryGenerationId, MemoryGenerationStatus, MemoryItemId, MemoryItemKind,
    MemoryItemStatus, MemoryScope, MemorySensitivity, OutboxId, PrincipalUserId, ProviderSubject,
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

    /// Stores a one-time identity link token hash. Raw tokens must not be passed here.
    pub async fn insert_link_token(&self, token: &LifeLinkToken) -> LifeStorageResult<()> {
        query::<Postgres>(
            r#"
            INSERT INTO life_link_tokens (
                token_hash, principal_user_id, target_provider, expires_at, consumed_at, created_at
            )
            VALUES ($1, $2, $3, $4, $5, $6)
            "#,
        )
        .bind(&token.token_hash)
        .bind(token.principal_user_id.get())
        .bind(token.target_provider.as_str())
        .bind(token.expires_at.get())
        .bind(token.consumed_at.map(TimestampMillis::get))
        .bind(token.created_at.get())
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(())
    }

    /// Atomically consumes a valid one-time link token and links the target provider subject.
    pub async fn consume_link_token(
        &self,
        token_hash: &str,
        provider: LifeIdentityProvider,
        provider_subject: &ProviderSubject,
        now: TimestampMillis,
    ) -> LifeStorageResult<Option<PrincipalUserId>> {
        let mut tx = self.pool.begin().await.map_err(db_error)?;
        let row = query::<Postgres>(
            r#"
            UPDATE life_link_tokens
            SET consumed_at = $3
            WHERE token_hash = $1
              AND target_provider = $2
              AND consumed_at IS NULL
              AND expires_at > $3
            RETURNING principal_user_id
            "#,
        )
        .bind(token_hash)
        .bind(provider.as_str())
        .bind(now.get())
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_error)?;
        let Some(row) = row else {
            tx.commit().await.map_err(db_error)?;
            return Ok(None);
        };
        let principal_user_id = PrincipalUserId::new(row.get::<i64, _>("principal_user_id"))?;

        let link_row = query::<Postgres>(
            r#"
            INSERT INTO life_identity_links (
                provider, provider_subject, principal_user_id, verified_at, created_at, updated_at
            )
            VALUES ($1, $2, $3, $4, $4, $4)
            ON CONFLICT (provider, provider_subject) DO UPDATE
            SET verified_at = EXCLUDED.verified_at,
                updated_at = EXCLUDED.updated_at
            WHERE life_identity_links.principal_user_id = EXCLUDED.principal_user_id
            RETURNING principal_user_id
            "#,
        )
        .bind(provider.as_str())
        .bind(provider_subject.as_str())
        .bind(principal_user_id.get())
        .bind(now.get())
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_error)?;
        if link_row.is_none() {
            return Err(LifeStorageError::IdentityLinkConflict {
                provider,
                provider_subject: provider_subject.clone(),
            });
        }

        tx.commit().await.map_err(db_error)?;
        Ok(Some(principal_user_id))
    }

    /// Lists memory generations for an inspector/manage API.
    pub async fn list_memory_generations(
        &self,
        principal_user_id: PrincipalUserId,
    ) -> LifeStorageResult<Vec<LifeMemoryGeneration>> {
        let rows = query::<Postgres>(
            r#"
            SELECT *
            FROM life_memory_generations
            WHERE principal_user_id = $1
            ORDER BY generation_number DESC
            "#,
        )
        .bind(principal_user_id.get())
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;

        rows.into_iter().map(generation_from_row).collect()
    }

    /// Lists recent canonical transcript turns for a principal.
    pub async fn list_turns(
        &self,
        principal_user_id: PrincipalUserId,
        limit: i64,
    ) -> LifeStorageResult<Vec<LifeTurn>> {
        let rows = query::<Postgres>(
            r#"
            SELECT *
            FROM life_turns
            WHERE principal_user_id = $1
            ORDER BY created_at DESC, turn_id ASC
            LIMIT $2
            "#,
        )
        .bind(principal_user_id.get())
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;

        rows.into_iter().map(turn_from_row).collect()
    }

    /// Lists recent run events for a principal.
    pub async fn list_events(
        &self,
        principal_user_id: PrincipalUserId,
        limit: i64,
    ) -> LifeStorageResult<Vec<LifeEvent>> {
        let rows = query::<Postgres>(
            r#"
            SELECT events.*
            FROM life_events events
            INNER JOIN life_runs runs ON runs.run_id = events.run_id
            WHERE runs.principal_user_id = $1
            ORDER BY events.created_at DESC, events.run_id ASC, events.seq DESC
            LIMIT $2
            "#,
        )
        .bind(principal_user_id.get())
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;

        rows.into_iter().map(event_from_row).collect()
    }

    /// Lists a cursor-paged window of canonical transcript turns for a principal.
    ///
    /// The cursor is an opaque string produced by a prior call to this method.
    /// `None` starts from the most recent turn. The page contains at most `limit`
    /// turns; `next_cursor` is `Some` when more rows exist beyond this page.
    pub async fn list_turns_page(
        &self,
        principal_user_id: PrincipalUserId,
        cursor: Option<&str>,
        limit: i64,
    ) -> LifeStorageResult<TurnsPage> {
        let fetch_limit = limit.saturating_add(1);
        let rows = if let Some(parsed) = parse_turn_cursor(cursor)? {
            query::<Postgres>(
                r#"
                SELECT *
                FROM life_turns
                WHERE principal_user_id = $1
                  AND (created_at < $2
                       OR (created_at = $2 AND turn_id > $3))
                ORDER BY created_at DESC, turn_id ASC
                LIMIT $4
                "#,
            )
            .bind(principal_user_id.get())
            .bind(parsed.created_at)
            .bind(parsed.turn_id)
            .bind(fetch_limit)
            .fetch_all(&self.pool)
            .await
            .map_err(db_error)?
        } else {
            query::<Postgres>(
                r#"
                SELECT *
                FROM life_turns
                WHERE principal_user_id = $1
                ORDER BY created_at DESC, turn_id ASC
                LIMIT $2
                "#,
            )
            .bind(principal_user_id.get())
            .bind(fetch_limit)
            .fetch_all(&self.pool)
            .await
            .map_err(db_error)?
        };

        let has_more = rows.len() > limit as usize;
        let turns: Vec<LifeTurn> = rows
            .into_iter()
            .take(limit as usize)
            .map(turn_from_row)
            .collect::<LifeStorageResult<Vec<_>>>()?;
        let next_cursor = if has_more {
            turns
                .last()
                .map(|turn| encode_turn_cursor(turn.created_at.get(), turn.turn_id))
        } else {
            None
        };

        Ok(TurnsPage { turns, next_cursor })
    }

    /// Lists a cursor-paged window of run events for a principal or a specific run.
    ///
    /// When `run_id` is `None`, events are scoped to the principal (all runs).
    /// When `run_id` is `Some`, events are scoped to that single run.
    /// The cursor is an opaque string produced by a prior call to this method.
    pub async fn list_events_page(
        &self,
        principal_user_id: PrincipalUserId,
        run_id: Option<RunId>,
        cursor: Option<&str>,
        limit: i64,
    ) -> LifeStorageResult<EventsPage> {
        let fetch_limit = limit.saturating_add(1);
        let parsed = parse_event_cursor(cursor)?;

        let rows = match (run_id, parsed) {
            (Some(rid), None) => query::<Postgres>(
                r#"
                SELECT *
                FROM life_events
                WHERE run_id = $1
                ORDER BY created_at DESC, run_id ASC, seq DESC
                LIMIT $2
                "#,
            )
            .bind(rid.as_uuid())
            .bind(fetch_limit)
            .fetch_all(&self.pool)
            .await
            .map_err(db_error)?,
            (Some(rid), Some(cur)) => query::<Postgres>(
                r#"
                SELECT *
                FROM life_events
                WHERE run_id = $1
                  AND (created_at < $2
                       OR (created_at = $2 AND run_id > $3)
                       OR (created_at = $2 AND run_id = $3 AND seq < $4))
                ORDER BY created_at DESC, run_id ASC, seq DESC
                LIMIT $5
                "#,
            )
            .bind(rid.as_uuid())
            .bind(cur.created_at)
            .bind(cur.run_id)
            .bind(cur.seq)
            .bind(fetch_limit)
            .fetch_all(&self.pool)
            .await
            .map_err(db_error)?,
            (None, None) => query::<Postgres>(
                r#"
                SELECT events.*
                FROM life_events events
                INNER JOIN life_runs runs ON runs.run_id = events.run_id
                WHERE runs.principal_user_id = $1
                ORDER BY events.created_at DESC, events.run_id ASC, events.seq DESC
                LIMIT $2
                "#,
            )
            .bind(principal_user_id.get())
            .bind(fetch_limit)
            .fetch_all(&self.pool)
            .await
            .map_err(db_error)?,
            (None, Some(cur)) => query::<Postgres>(
                r#"
                SELECT events.*
                FROM life_events events
                INNER JOIN life_runs runs ON runs.run_id = events.run_id
                WHERE runs.principal_user_id = $1
                  AND (events.created_at < $2
                       OR (events.created_at = $2 AND events.run_id > $3)
                       OR (events.created_at = $2 AND events.run_id = $3 AND events.seq < $4))
                ORDER BY events.created_at DESC, events.run_id ASC, events.seq DESC
                LIMIT $5
                "#,
            )
            .bind(principal_user_id.get())
            .bind(cur.created_at)
            .bind(cur.run_id)
            .bind(cur.seq)
            .bind(fetch_limit)
            .fetch_all(&self.pool)
            .await
            .map_err(db_error)?,
        };

        let has_more = rows.len() > limit as usize;
        let events: Vec<LifeEvent> = rows
            .into_iter()
            .take(limit as usize)
            .map(event_from_row)
            .collect::<LifeStorageResult<Vec<_>>>()?;
        let next_cursor = if has_more {
            events
                .last()
                .map(|event| encode_event_cursor(event.created_at.get(), event.run_id, event.seq))
        } else {
            None
        };

        Ok(EventsPage {
            events,
            next_cursor,
        })
    }

    /// Lists canonical memory items for a specific generation scope, including candidates/deleted rows.
    pub async fn memory_items_for_generation(
        &self,
        scope: MemoryScope,
    ) -> LifeStorageResult<Vec<LifeMemoryItem>> {
        let rows = query::<Postgres>(
            r#"
            SELECT *
            FROM life_memory_items
            WHERE principal_user_id = $1 AND memory_generation_id = $2
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

    /// Marks one active/candidate memory row deleted for explicit user forget.
    pub async fn mark_memory_item_deleted(
        &self,
        principal_user_id: PrincipalUserId,
        memory_id: MemoryItemId,
        now: TimestampMillis,
    ) -> LifeStorageResult<bool> {
        let result = query::<Postgres>(
            r#"
            UPDATE life_memory_items
            SET status = 'deleted', updated_at = $3
            WHERE principal_user_id = $1
              AND memory_id = $2
              AND status IN ('active', 'candidate')
            "#,
        )
        .bind(principal_user_id.get())
        .bind(memory_id.as_uuid())
        .bind(now.get())
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(result.rows_affected() > 0)
    }

    /// Creates an inactive near-empty generation, copying only explicitly selected seed memories.
    pub async fn soft_reset_memory_generation(
        &self,
        principal_user_id: PrincipalUserId,
        seed_memory_ids: &[MemoryItemId],
        now: TimestampMillis,
        reason: &str,
    ) -> LifeStorageResult<LifeMemoryGeneration> {
        let source_generation = self.active_generation(principal_user_id).await?;
        let generation_number = self
            .next_memory_generation_number(principal_user_id)
            .await?;
        let generation = LifeMemoryGeneration {
            memory_generation_id: MemoryGenerationId::new_v4(),
            principal_user_id,
            generation_number,
            status: MemoryGenerationStatus::Building,
            source_generation_id: source_generation.map(|active| active.scope.memory_generation_id),
            build_reason: reason.to_owned(),
            build_policy: serde_json::json!({"mode": "soft_reset", "version": 1}),
            source_scope: serde_json::json!({
                "seed_memory_ids": seed_memory_ids.iter().map(ToString::to_string).collect::<Vec<_>>()
            }),
            comparison_report: serde_json::json!({"status": "pending_activation"}),
            activated_at: None,
            created_at: now,
            updated_at: now,
        };
        self.insert_memory_generation(&generation).await?;

        if let Some(active) = source_generation {
            for seed in self
                .active_memory_items_by_ids(active.scope, seed_memory_ids)
                .await?
            {
                let mut clone = seed;
                clone.memory_id = MemoryItemId::new_v4();
                clone.memory_generation_id = generation.memory_generation_id;
                clone.created_at = now;
                clone.updated_at = now;
                self.upsert_memory_item(&clone).await?;
            }
        }

        Ok(generation)
    }

    /// Deletes derived Engram delivery state for one generation without touching canonical memory.
    pub async fn wipe_derived_generation(
        &self,
        principal_user_id: PrincipalUserId,
        memory_generation_id: MemoryGenerationId,
    ) -> LifeStorageResult<u64> {
        let result = query::<Postgres>(
            r#"
            DELETE FROM life_engram_outbox
            WHERE principal_user_id = $1 AND memory_generation_id = $2
            "#,
        )
        .bind(principal_user_id.get())
        .bind(memory_generation_id.as_uuid())
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(result.rows_affected())
    }

    /// Wipes materialized rows for an inactive generation and marks the generation deleted.
    pub async fn wipe_memory_generation(
        &self,
        principal_user_id: PrincipalUserId,
        memory_generation_id: MemoryGenerationId,
        now: TimestampMillis,
    ) -> LifeStorageResult<()> {
        let mut tx = self.pool.begin().await.map_err(db_error)?;
        let generation = query::<Postgres>(
            r#"
            SELECT *
            FROM life_memory_generations
            WHERE principal_user_id = $1 AND memory_generation_id = $2
            "#,
        )
        .bind(principal_user_id.get())
        .bind(memory_generation_id.as_uuid())
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_error)?;
        let Some(generation) = generation else {
            return Err(LifeStorageError::GenerationNotOwned {
                principal_user_id,
                generation_id: memory_generation_id,
            });
        };
        match generation_status_from_str(generation.get::<&str, _>("status"))? {
            MemoryGenerationStatus::Active => {
                return Err(LifeStorageError::GenerationIsActive {
                    principal_user_id,
                    generation_id: memory_generation_id,
                });
            }
            MemoryGenerationStatus::Deleted => {
                return Err(LifeStorageError::GenerationDeleted {
                    principal_user_id,
                    generation_id: memory_generation_id,
                });
            }
            MemoryGenerationStatus::Building
            | MemoryGenerationStatus::Archived
            | MemoryGenerationStatus::Failed => {}
        }

        for table in [
            "life_engram_outbox",
            "life_memory_items",
            "life_task_states",
            "life_friction_patterns",
            "life_support_protocols",
        ] {
            let sql = format!(
                "DELETE FROM {table} WHERE principal_user_id = $1 AND memory_generation_id = $2"
            );
            query::<Postgres>(&sql)
                .bind(principal_user_id.get())
                .bind(memory_generation_id.as_uuid())
                .execute(&mut *tx)
                .await
                .map_err(db_error)?;
        }

        query::<Postgres>(
            r#"
            UPDATE life_memory_generations
            SET status = 'deleted', updated_at = $3
            WHERE principal_user_id = $1 AND memory_generation_id = $2
            "#,
        )
        .bind(principal_user_id.get())
        .bind(memory_generation_id.as_uuid())
        .bind(now.get())
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;
        tx.commit().await.map_err(db_error)
    }

    /// Irreversibly deletes life-owned state and the stable life checkpoint, preserving `users`.
    pub async fn privacy_hard_wipe_life_state(
        &self,
        principal_user_id: PrincipalUserId,
    ) -> LifeStorageResult<()> {
        let mut tx = self.pool.begin().await.map_err(db_error)?;
        query::<Postgres>(
            r#"
            DELETE FROM agent_memory_snapshots
            WHERE user_id = $1 AND context_key = $2 AND flow_id = $3
            "#,
        )
        .bind(principal_user_id.get())
        .bind(crate::worker::LIFE_CONTEXT_KEY)
        .bind(crate::worker::LIFE_FLOW_ID)
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;
        query::<Postgres>("DELETE FROM life_principals WHERE principal_user_id = $1")
            .bind(principal_user_id.get())
            .execute(&mut *tx)
            .await
            .map_err(db_error)?;
        tx.commit().await.map_err(db_error)
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
        let status_row = query::<Postgres>(
            r#"
            SELECT status
            FROM life_memory_generations
            WHERE principal_user_id = $1 AND memory_generation_id = $2
            "#,
        )
        .bind(principal_user_id.get())
        .bind(memory_generation_id.as_uuid())
        .fetch_one(&mut *tx)
        .await
        .map_err(db_error)?;
        if generation_status_from_str(status_row.get::<&str, _>("status"))?
            == MemoryGenerationStatus::Deleted
        {
            return Err(LifeStorageError::GenerationDeleted {
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

    async fn insert_engram_outbox(&self, row: &LifeEngramOutboxRow) -> LifeStorageResult<()> {
        query::<Postgres>(
            r#"
            INSERT INTO life_engram_outbox (
                outbox_id, principal_user_id, memory_generation_id, source_memory_id,
                idempotency_key, payload, status, attempts, next_attempt_at, last_error,
                created_at, updated_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
            ON CONFLICT (idempotency_key) DO NOTHING
            "#,
        )
        .bind(row.outbox_id.as_uuid())
        .bind(row.principal_user_id.get())
        .bind(row.memory_generation_id.as_uuid())
        .bind(row.source_memory_id.map(MemoryItemId::as_uuid))
        .bind(&row.idempotency_key)
        .bind(&row.payload)
        .bind(engram_outbox_status_as_str(row.status))
        .bind(row.attempts)
        .bind(row.next_attempt_at.get())
        .bind(&row.last_error)
        .bind(row.created_at.get())
        .bind(row.updated_at.get())
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(())
    }

    async fn claim_due_engram_outbox(
        &self,
        limit: i64,
        now: TimestampMillis,
    ) -> LifeStorageResult<Vec<LifeEngramOutboxRow>> {
        let rows = query::<Postgres>(
            r#"
            UPDATE life_engram_outbox
            SET status = 'flushing',
                attempts = attempts + 1,
                updated_at = $2
            WHERE outbox_id IN (
                SELECT outbox_id
                FROM life_engram_outbox
                WHERE status = 'pending'
                  AND next_attempt_at <= $1
                ORDER BY next_attempt_at ASC, created_at ASC, outbox_id ASC
                LIMIT $3
                FOR UPDATE SKIP LOCKED
            )
            RETURNING *
            "#,
        )
        .bind(now.get())
        .bind(now.get())
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;

        rows.into_iter().map(engram_outbox_from_row).collect()
    }

    async fn mark_engram_outbox_flushed(
        &self,
        outbox_id: OutboxId,
        now: TimestampMillis,
    ) -> LifeStorageResult<()> {
        query::<Postgres>(
            r#"
            UPDATE life_engram_outbox
            SET status = 'flushed',
                last_error = NULL,
                updated_at = $2
            WHERE outbox_id = $1
            "#,
        )
        .bind(outbox_id.as_uuid())
        .bind(now.get())
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(())
    }

    async fn mark_engram_outbox_retry(
        &self,
        outbox_id: OutboxId,
        last_error: &str,
        next_attempt_at: TimestampMillis,
        now: TimestampMillis,
    ) -> LifeStorageResult<()> {
        query::<Postgres>(
            r#"
            UPDATE life_engram_outbox
            SET status = 'pending',
                last_error = $2,
                next_attempt_at = $3,
                updated_at = $4
            WHERE outbox_id = $1
            "#,
        )
        .bind(outbox_id.as_uuid())
        .bind(last_error)
        .bind(next_attempt_at.get())
        .bind(now.get())
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(())
    }

    async fn mark_engram_outbox_dead(
        &self,
        outbox_id: OutboxId,
        last_error: &str,
        now: TimestampMillis,
    ) -> LifeStorageResult<()> {
        query::<Postgres>(
            r#"
            UPDATE life_engram_outbox
            SET status = 'dead',
                last_error = $2,
                updated_at = $3
            WHERE outbox_id = $1
            "#,
        )
        .bind(outbox_id.as_uuid())
        .bind(last_error)
        .bind(now.get())
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

    async fn active_memory_items_by_ids(
        &self,
        scope: MemoryScope,
        memory_ids: &[MemoryItemId],
    ) -> LifeStorageResult<Vec<LifeMemoryItem>> {
        if memory_ids.is_empty() {
            return Ok(Vec::new());
        }
        let ids = memory_ids
            .iter()
            .map(|memory_id| memory_id.as_uuid())
            .collect::<Vec<_>>();
        let rows = query::<Postgres>(
            r#"
            SELECT *
            FROM life_memory_items
            WHERE principal_user_id = $1
              AND memory_generation_id = $2
              AND memory_id = ANY($3)
              AND status = 'active'
            ORDER BY updated_at DESC, memory_id ASC
            "#,
        )
        .bind(scope.principal_user_id.get())
        .bind(scope.memory_generation_id.as_uuid())
        .bind(&ids)
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

fn redaction_state_from_str(value: &str) -> LifeStorageResult<RedactionState> {
    match value {
        "clean" => Ok(RedactionState::Clean),
        "redacted" => Ok(RedactionState::Redacted),
        "secret_blocked" => Ok(RedactionState::SecretBlocked),
        other => unknown_enum("redaction_state", other),
    }
}

fn turn_role_from_str(value: &str) -> LifeStorageResult<LifeTurnRole> {
    match value {
        "user" => Ok(LifeTurnRole::User),
        "assistant" => Ok(LifeTurnRole::Assistant),
        "system" => Ok(LifeTurnRole::System),
        "tool" => Ok(LifeTurnRole::Tool),
        other => unknown_enum("life_turn_role", other),
    }
}

fn source_transport_from_str(value: &str) -> LifeStorageResult<LifeSourceTransport> {
    match value {
        "web" => Ok(LifeSourceTransport::Web),
        "telegram" => Ok(LifeSourceTransport::Telegram),
        "internal" => Ok(LifeSourceTransport::Internal),
        other => unknown_enum("life_source_transport", other),
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

fn generation_status_from_str(value: &str) -> LifeStorageResult<MemoryGenerationStatus> {
    match value {
        "building" => Ok(MemoryGenerationStatus::Building),
        "active" => Ok(MemoryGenerationStatus::Active),
        "archived" => Ok(MemoryGenerationStatus::Archived),
        "failed" => Ok(MemoryGenerationStatus::Failed),
        "deleted" => Ok(MemoryGenerationStatus::Deleted),
        other => unknown_enum("memory_generation_status", other),
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

fn engram_outbox_status_as_str(status: LifeEngramOutboxStatus) -> &'static str {
    match status {
        LifeEngramOutboxStatus::Pending => "pending",
        LifeEngramOutboxStatus::Flushing => "flushing",
        LifeEngramOutboxStatus::Flushed => "flushed",
        LifeEngramOutboxStatus::Dead => "dead",
    }
}

fn engram_outbox_status_from_str(value: &str) -> LifeStorageResult<LifeEngramOutboxStatus> {
    match value {
        "pending" => Ok(LifeEngramOutboxStatus::Pending),
        "flushing" => Ok(LifeEngramOutboxStatus::Flushing),
        "flushed" => Ok(LifeEngramOutboxStatus::Flushed),
        "dead" => Ok(LifeEngramOutboxStatus::Dead),
        other => unknown_enum("life_engram_outbox_status", other),
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

fn generation_from_row(row: sqlx_postgres::PgRow) -> LifeStorageResult<LifeMemoryGeneration> {
    Ok(LifeMemoryGeneration {
        memory_generation_id: MemoryGenerationId::from_uuid(row.get("memory_generation_id")),
        principal_user_id: PrincipalUserId::new(row.get("principal_user_id"))?,
        generation_number: row.get("generation_number"),
        status: generation_status_from_str(row.get::<&str, _>("status"))?,
        source_generation_id: row
            .get::<Option<Uuid>, _>("source_generation_id")
            .map(MemoryGenerationId::from_uuid),
        build_reason: row.get("build_reason"),
        build_policy: row.get("build_policy"),
        source_scope: row.get("source_scope"),
        comparison_report: row.get("comparison_report"),
        activated_at: row
            .get::<Option<i64>, _>("activated_at")
            .map(TimestampMillis::new),
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

fn turn_from_row(row: sqlx_postgres::PgRow) -> LifeStorageResult<LifeTurn> {
    Ok(LifeTurn {
        turn_id: TurnId::from_uuid(row.get("turn_id")),
        principal_user_id: PrincipalUserId::new(row.get("principal_user_id"))?,
        run_id: row.get::<Option<Uuid>, _>("run_id").map(RunId::from_uuid),
        role: turn_role_from_str(row.get::<&str, _>("role"))?,
        source_transport: source_transport_from_str(row.get::<&str, _>("source_transport"))?,
        source_ref: row.get("source_ref"),
        content: row.get("content"),
        attachments: row.get("attachments"),
        transport_metadata: row.get("transport_metadata"),
        redaction_state: redaction_state_from_str(row.get::<&str, _>("redaction_state"))?,
        created_at: TimestampMillis::new(row.get("created_at")),
    })
}

fn event_from_row(row: sqlx_postgres::PgRow) -> LifeStorageResult<LifeEvent> {
    Ok(LifeEvent {
        event_id: crate::domain::EventId::from_uuid(row.get("event_id")),
        run_id: RunId::from_uuid(row.get("run_id")),
        seq: row.get("seq"),
        kind: row.get("kind"),
        payload: row.get("payload"),
        created_at: TimestampMillis::new(row.get("created_at")),
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

fn engram_outbox_from_row(row: sqlx_postgres::PgRow) -> LifeStorageResult<LifeEngramOutboxRow> {
    Ok(LifeEngramOutboxRow {
        outbox_id: OutboxId::from_uuid(row.get("outbox_id")),
        principal_user_id: PrincipalUserId::new(row.get("principal_user_id"))?,
        memory_generation_id: MemoryGenerationId::from_uuid(row.get("memory_generation_id")),
        source_memory_id: row
            .get::<Option<Uuid>, _>("source_memory_id")
            .map(MemoryItemId::from_uuid),
        idempotency_key: row.get("idempotency_key"),
        payload: row.get("payload"),
        status: engram_outbox_status_from_str(row.get::<&str, _>("status"))?,
        attempts: row.get("attempts"),
        next_attempt_at: TimestampMillis::new(row.get("next_attempt_at")),
        last_error: row.get("last_error"),
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

// ---------------------------------------------------------------------------
// Cursor-paged response types
// ---------------------------------------------------------------------------

/// A cursor-paged page of canonical transcript turns.
#[derive(Debug, Clone, PartialEq)]
pub struct TurnsPage {
    /// Turns in this page, ordered newest-first.
    pub turns: Vec<LifeTurn>,
    /// Opaque cursor for the next page, or `None` if this is the last page.
    pub next_cursor: Option<String>,
}

/// A cursor-paged page of run events.
#[derive(Debug, Clone, PartialEq)]
pub struct EventsPage {
    /// Events in this page, ordered newest-first.
    pub events: Vec<LifeEvent>,
    /// Opaque cursor for the next page, or `None` if this is the last page.
    pub next_cursor: Option<String>,
}

// ---------------------------------------------------------------------------
// Cursor encoding / decoding (opaque to callers)
// ---------------------------------------------------------------------------

struct TurnCursor {
    created_at: i64,
    turn_id: Uuid,
}

struct EventCursor {
    created_at: i64,
    run_id: Uuid,
    seq: i64,
}

fn encode_turn_cursor(created_at: i64, turn_id: TurnId) -> String {
    format!("{created_at}:{turn_id}")
}

fn encode_event_cursor(created_at: i64, run_id: RunId, seq: i64) -> String {
    format!("{created_at}:{run_id}:{seq}")
}

fn parse_turn_cursor(cursor: Option<&str>) -> LifeStorageResult<Option<TurnCursor>> {
    let Some(cursor) = cursor else {
        return Ok(None);
    };
    let cursor = cursor.trim();
    if cursor.is_empty() {
        return Ok(None);
    }
    let (created_at_str, turn_id_str) = cursor
        .split_once(':')
        .ok_or_else(|| LifeStorageError::InvalidCursor(cursor.to_owned()))?;
    let created_at: i64 = created_at_str
        .parse()
        .map_err(|_| LifeStorageError::InvalidCursor(cursor.to_owned()))?;
    let turn_id: Uuid = turn_id_str
        .parse()
        .map_err(|_| LifeStorageError::InvalidCursor(cursor.to_owned()))?;
    Ok(Some(TurnCursor {
        created_at,
        turn_id,
    }))
}

fn parse_event_cursor(cursor: Option<&str>) -> LifeStorageResult<Option<EventCursor>> {
    let Some(cursor) = cursor else {
        return Ok(None);
    };
    let cursor = cursor.trim();
    if cursor.is_empty() {
        return Ok(None);
    }
    let parts: Vec<&str> = cursor.splitn(3, ':').collect();
    if parts.len() != 3 {
        return Err(LifeStorageError::InvalidCursor(cursor.to_owned()));
    }
    let created_at: i64 = parts[0]
        .parse()
        .map_err(|_| LifeStorageError::InvalidCursor(cursor.to_owned()))?;
    let run_id: Uuid = parts[1]
        .parse()
        .map_err(|_| LifeStorageError::InvalidCursor(cursor.to_owned()))?;
    let seq: i64 = parts[2]
        .parse()
        .map_err(|_| LifeStorageError::InvalidCursor(cursor.to_owned()))?;
    Ok(Some(EventCursor {
        created_at,
        run_id,
        seq,
    }))
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
    use crate::linking::{RawLifeLinkToken, hash_link_token};

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
        let outbox_id = crate::domain::OutboxId::new_v4();
        must(
            storage
                .insert_engram_outbox(&LifeEngramOutboxRow {
                    outbox_id,
                    principal_user_id,
                    memory_generation_id: active.scope.memory_generation_id,
                    source_memory_id: Some(memories[0].memory_id),
                    idempotency_key: format!("test-outbox-{}", outbox_id),
                    payload: json!({"external_id": memories[0].memory_id.to_string()}),
                    status: LifeEngramOutboxStatus::Pending,
                    attempts: 0,
                    next_attempt_at: now,
                    last_error: None,
                    created_at: now,
                    updated_at: now,
                })
                .await,
            "insert outbox projection",
        );
        let outbox_row = must(
            query::<Postgres>(
                r#"
                SELECT source_memory_id, status
                FROM life_engram_outbox
                WHERE outbox_id = $1
                  AND principal_user_id = $2
                  AND memory_generation_id = $3
                "#,
            )
            .bind(outbox_id.as_uuid())
            .bind(principal_user_id.get())
            .bind(active.scope.memory_generation_id.as_uuid())
            .fetch_one(storage.pool())
            .await,
            "load outbox projection",
        );
        assert_eq!(
            outbox_row.get::<Option<Uuid>, _>("source_memory_id"),
            Some(memories[0].memory_id.as_uuid())
        );
        assert_eq!(outbox_row.get::<String, _>("status"), "pending");
        let dereferenced = must(
            storage
                .active_memory_items_by_ids(
                    active.scope,
                    &[memories[0].memory_id, MemoryItemId::new_v4()],
                )
                .await,
            "load memory by ids",
        );
        assert_eq!(dereferenced.len(), 1);
        assert_eq!(dereferenced[0].memory_id, memories[0].memory_id);
        let claimed_outbox = must(
            storage.claim_due_engram_outbox(10, now).await,
            "claim due outbox",
        );
        assert_eq!(claimed_outbox.len(), 1);
        assert_eq!(claimed_outbox[0].outbox_id, outbox_id);
        assert_eq!(claimed_outbox[0].status, LifeEngramOutboxStatus::Flushing);
        assert_eq!(claimed_outbox[0].attempts, 1);
        must(
            storage
                .mark_engram_outbox_retry(
                    outbox_id,
                    "temporary backend failure",
                    TimestampMillis::new(now.get() + 500),
                    TimestampMillis::new(now.get() + 10),
                )
                .await,
            "requeue outbox",
        );
        let not_due = must(
            storage
                .claim_due_engram_outbox(10, TimestampMillis::new(now.get() + 100))
                .await,
            "claim not due outbox",
        );
        assert!(not_due.is_empty());
        let claimed_outbox = must(
            storage
                .claim_due_engram_outbox(10, TimestampMillis::new(now.get() + 500))
                .await,
            "claim due retry outbox",
        );
        assert_eq!(claimed_outbox.len(), 1);
        assert_eq!(claimed_outbox[0].attempts, 2);
        must(
            storage
                .mark_engram_outbox_flushed(outbox_id, TimestampMillis::new(now.get() + 600))
                .await,
            "mark outbox flushed",
        );
        let flushed_status = must(
            query::<Postgres>("SELECT status FROM life_engram_outbox WHERE outbox_id = $1")
                .bind(outbox_id.as_uuid())
                .fetch_one(storage.pool())
                .await,
            "load flushed outbox status",
        );
        assert_eq!(flushed_status.get::<String, _>("status"), "flushed");
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
            sensitivity: crate::gateway::LifeInputSensitivity::Normal,
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
    async fn sqlx_life_link_tokens_and_wipe_lifecycle_are_db_backed() {
        let Some(storage) = sqlx_test_storage().await else {
            return;
        };
        let principal_user_id = unique_principal_user_id();
        let now = TimestampMillis::new(1_700_000_200_000);
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

        let raw_token = RawLifeLinkToken::new(format!("link-token-{}", principal_user_id.get()));
        let token_hash = hash_link_token(&raw_token);
        must(
            storage
                .insert_link_token(&LifeLinkToken {
                    token_hash: token_hash.clone(),
                    principal_user_id,
                    target_provider: LifeIdentityProvider::Telegram,
                    expires_at: TimestampMillis::new(now.get() + 60_000),
                    consumed_at: None,
                    created_at: now,
                })
                .await,
            "insert link token",
        );
        let telegram_subject = must(ProviderSubject::new("telegram-link-target"), "subject");
        let linked = must(
            storage
                .consume_link_token(
                    &token_hash,
                    LifeIdentityProvider::Telegram,
                    &telegram_subject,
                    TimestampMillis::new(now.get() + 1),
                )
                .await,
            "consume token",
        );
        assert_eq!(linked, Some(principal_user_id));
        assert_eq!(
            must(
                storage
                    .resolve_identity(LifeIdentityProvider::Telegram, &telegram_subject)
                    .await,
                "resolve linked telegram identity",
            ),
            Some(principal_user_id)
        );
        let reused = must(
            storage
                .consume_link_token(
                    &token_hash,
                    LifeIdentityProvider::Telegram,
                    &telegram_subject,
                    TimestampMillis::new(now.get() + 2),
                )
                .await,
            "reuse token",
        );
        assert_eq!(reused, None);

        let gen1 = generation(principal_user_id, 1, MemoryGenerationStatus::Building, now);
        must(storage.insert_memory_generation(&gen1).await, "insert gen1");
        let active = must(
            storage
                .activate_memory_generation(
                    principal_user_id,
                    gen1.memory_generation_id,
                    now,
                    "active",
                )
                .await,
            "activate gen1",
        );
        let seed = memory_item(
            principal_user_id,
            active.scope.memory_generation_id,
            "seed memory",
            now,
        );
        let seed_id = seed.memory_id;
        must(storage.upsert_memory_item(&seed).await, "insert seed");
        let outbox_id = OutboxId::new_v4();
        must(
            storage
                .insert_engram_outbox(&LifeEngramOutboxRow {
                    outbox_id,
                    principal_user_id,
                    memory_generation_id: active.scope.memory_generation_id,
                    source_memory_id: Some(seed_id),
                    idempotency_key: format!("wipe-test-{outbox_id}"),
                    payload: json!({"external_id": seed_id.to_string()}),
                    status: LifeEngramOutboxStatus::Pending,
                    attempts: 0,
                    next_attempt_at: now,
                    last_error: None,
                    created_at: now,
                    updated_at: now,
                })
                .await,
            "insert outbox",
        );
        assert_eq!(
            must(
                storage
                    .wipe_derived_generation(principal_user_id, active.scope.memory_generation_id)
                    .await,
                "derived wipe",
            ),
            1
        );
        assert_eq!(
            must(
                storage.active_memory_items(active.scope).await,
                "memory after derived wipe"
            )
            .len(),
            1
        );

        let reset_generation = must(
            storage
                .soft_reset_memory_generation(
                    principal_user_id,
                    &[seed_id],
                    TimestampMillis::new(now.get() + 3),
                    "soft reset",
                )
                .await,
            "soft reset generation",
        );
        assert_eq!(reset_generation.status, MemoryGenerationStatus::Building);
        assert_ne!(
            reset_generation.memory_generation_id,
            active.scope.memory_generation_id
        );
        assert_eq!(
            must(
                storage.active_generation(principal_user_id).await,
                "active unchanged"
            ),
            Some(active)
        );
        let reset_scope =
            MemoryScope::new(principal_user_id, reset_generation.memory_generation_id);
        let reset_memories = must(
            storage.active_memory_items(reset_scope).await,
            "seed copied into reset generation",
        );
        assert_eq!(reset_memories.len(), 1);
        assert_eq!(reset_memories[0].text, "seed memory");
        assert_ne!(reset_memories[0].memory_id, seed_id);

        let active_wipe_error = storage
            .wipe_memory_generation(
                principal_user_id,
                active.scope.memory_generation_id,
                TimestampMillis::new(now.get() + 4),
            )
            .await
            .expect_err("active generation must not be wiped");
        assert!(matches!(
            active_wipe_error,
            LifeStorageError::GenerationIsActive { .. }
        ));
        must(
            storage
                .wipe_memory_generation(
                    principal_user_id,
                    reset_generation.memory_generation_id,
                    TimestampMillis::new(now.get() + 5),
                )
                .await,
            "wipe inactive reset generation",
        );
        let generations = must(
            storage.list_memory_generations(principal_user_id).await,
            "list generations",
        );
        assert!(generations.iter().any(|generation| {
            generation.memory_generation_id == reset_generation.memory_generation_id
                && generation.status == MemoryGenerationStatus::Deleted
        }));

        must(
            storage
                .save_life_memory_checkpoint(
                    principal_user_id,
                    crate::worker::LIFE_CONTEXT_KEY,
                    crate::worker::LIFE_FLOW_ID,
                    &json!({"checkpoint": true}),
                    1,
                    now,
                )
                .await,
            "save checkpoint",
        );
        must(
            storage
                .privacy_hard_wipe_life_state(principal_user_id)
                .await,
            "privacy hard wipe",
        );
        assert!(
            must(
                storage.principal(principal_user_id).await,
                "principal after hard wipe"
            )
            .is_none()
        );
        let user_exists = must(
            query::<Postgres>("SELECT 1 FROM users WHERE user_id = $1")
                .bind(principal_user_id.get())
                .fetch_optional(storage.pool())
                .await,
            "load backing user row",
        )
        .is_some();
        assert!(user_exists, "hard wipe must not delete shared users row");
        let checkpoint_exists = must(
            query::<Postgres>(
                r#"
                SELECT 1
                FROM agent_memory_snapshots
                WHERE user_id = $1 AND context_key = $2 AND flow_id = $3
                "#,
            )
            .bind(principal_user_id.get())
            .bind(crate::worker::LIFE_CONTEXT_KEY)
            .bind(crate::worker::LIFE_FLOW_ID)
            .fetch_optional(storage.pool())
            .await,
            "load checkpoint after hard wipe",
        )
        .is_some();
        assert!(!checkpoint_exists);
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

    #[tokio::test]
    async fn sqlx_life_turns_and_events_cursor_paging() {
        let Some(storage) = sqlx_test_storage().await else {
            return;
        };
        let principal_user_id = unique_principal_user_id();
        let base = TimestampMillis::new(1_700_000_500_000);
        let principal = LifePrincipal {
            principal_user_id,
            profile_state: json!({}),
            operating_profile: json!({}),
            settings: json!({}),
            schema_version: 1,
            created_at: base,
            updated_at: base,
        };
        must(
            storage.upsert_principal(&principal).await,
            "upsert principal",
        );
        let gen_record = generation(principal_user_id, 1, MemoryGenerationStatus::Building, base);
        must(
            storage.insert_memory_generation(&gen_record).await,
            "insert generation",
        );
        must(
            storage
                .activate_memory_generation(
                    principal_user_id,
                    gen_record.memory_generation_id,
                    base,
                    "paging test",
                )
                .await,
            "activate generation",
        );

        // Append 5 turns with strictly increasing timestamps.
        let mut turns = Vec::new();
        for i in 0..5 {
            let turn = LifeTurn {
                turn_id: TurnId::new_v4(),
                principal_user_id,
                run_id: None,
                role: LifeTurnRole::User,
                source_transport: LifeSourceTransport::Internal,
                source_ref: None,
                content: format!("turn-{i}"),
                attachments: json!([]),
                transport_metadata: json!({}),
                redaction_state: RedactionState::Clean,
                created_at: TimestampMillis::new(base.get() + i),
            };
            must(
                storage.append_turn(&turn).await,
                &format!("append turn {i}"),
            );
            turns.push(turn);
        }

        // Page 1: limit=2 → newest two turns (turn-4, turn-3) + next_cursor.
        let page1 = must(
            storage.list_turns_page(principal_user_id, None, 2).await,
            "page1",
        );
        assert_eq!(page1.turns.len(), 2);
        assert_eq!(page1.turns[0].content, "turn-4");
        assert_eq!(page1.turns[1].content, "turn-3");
        let cursor1 = page1.next_cursor.expect("page1 should have next cursor");

        // Page 2: using cursor → next two turns (turn-2, turn-1) + next_cursor.
        let page2 = must(
            storage
                .list_turns_page(principal_user_id, Some(&cursor1), 2)
                .await,
            "page2",
        );
        assert_eq!(page2.turns.len(), 2);
        assert_eq!(page2.turns[0].content, "turn-2");
        assert_eq!(page2.turns[1].content, "turn-1");
        let cursor2 = page2.next_cursor.expect("page2 should have next cursor");

        // Page 3: using cursor → last turn (turn-0) + no next_cursor.
        let page3 = must(
            storage
                .list_turns_page(principal_user_id, Some(&cursor2), 2)
                .await,
            "page3",
        );
        assert_eq!(page3.turns.len(), 1);
        assert_eq!(page3.turns[0].content, "turn-0");
        assert!(page3.next_cursor.is_none(), "page3 should be the last");

        // Invalid cursor → InvalidCursor error.
        let bad = storage
            .list_turns_page(principal_user_id, Some("not-a-cursor"), 2)
            .await
            .expect_err("malformed cursor should fail");
        assert!(matches!(bad, LifeStorageError::InvalidCursor(_)));

        // Events paging: create a run and append events.
        let run_id = RunId::new_v4();
        let active = must(
            storage.active_generation(principal_user_id).await,
            "load active generation",
        )
        .expect("active generation should exist");
        must(
            query::<Postgres>(
                r#"
                INSERT INTO life_runs (
                    run_id, principal_user_id, memory_generation_id, status,
                    started_at, finished_at, last_checkpoint_at, error_text, created_at, updated_at
                )
                VALUES ($1, $2, $3, 'completed', $4, $4, $4, NULL, $4, $4)
                "#,
            )
            .bind(run_id.as_uuid())
            .bind(principal_user_id.get())
            .bind(active.scope.memory_generation_id.as_uuid())
            .bind(base.get())
            .execute(storage.pool())
            .await
            .map_err(db_error),
            "insert run",
        );

        for i in 0..4 {
            let seq = must(storage.next_event_seq(run_id).await, &format!("seq {i}"));
            must(
                storage
                    .append_event(&LifeEvent {
                        event_id: EventId::new_v4(),
                        run_id,
                        seq,
                        kind: format!("kind-{i}"),
                        payload: json!({"i": i}),
                        created_at: TimestampMillis::new(base.get() + i * 10),
                    })
                    .await,
                &format!("append event {i}"),
            );
        }

        // Events page 1 by run_id: limit=2 → newest two events.
        let epage1 = must(
            storage
                .list_events_page(principal_user_id, Some(run_id), None, 2)
                .await,
            "events page1",
        );
        assert_eq!(epage1.events.len(), 2);
        assert_eq!(epage1.events[0].kind, "kind-3");
        assert_eq!(epage1.events[1].kind, "kind-2");
        let ecursor1 = epage1.next_cursor.expect("events page1 next cursor");

        // Events page 2 by run_id: remaining two events.
        let epage2 = must(
            storage
                .list_events_page(principal_user_id, Some(run_id), Some(&ecursor1), 2)
                .await,
            "events page2",
        );
        assert_eq!(epage2.events.len(), 2);
        assert_eq!(epage2.events[0].kind, "kind-1");
        assert_eq!(epage2.events[1].kind, "kind-0");
        assert!(epage2.next_cursor.is_none(), "events page2 should be last");

        // Events by principal (no run_id) returns all 4 in one page.
        let epage_all = must(
            storage
                .list_events_page(principal_user_id, None, None, 100)
                .await,
            "events all by principal",
        );
        assert_eq!(epage_all.events.len(), 4);
        assert!(epage_all.next_cursor.is_none());
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
