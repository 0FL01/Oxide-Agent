//! SQLx/Postgres implementation for life-mode source-of-truth storage.

use std::path::Path;

use async_trait::async_trait;
use serde_json::Value;
use sqlx_core::migrate::Migrator;
use sqlx_core::query::query;
use sqlx_core::row::Row;
use sqlx_postgres::{PgPool, Postgres};
use uuid::Uuid;

use crate::domain::{
    BindingId, ClaimedLifeDelivery, DeliveryId, InputId, LifeDeliveryOutbox, LifeDeliveryStatus,
    LifeEvent, LifeIdentityLink, LifeInput, LifeInputStatus, LifePrincipal, LifeRun, LifeRunStatus,
    LifeTransportBinding, LifeTransportId, LifeTurn, LifeTurnRole, PrincipalUserId,
    ProviderSubject, RedactionState, RunId, TimestampMillis, TurnId, validate_delivery_worker_id,
};
use crate::storage::{
    ClaimedLifeInputRun, LIFE_DELIVERY_CLAIM_MILLIS, LIFE_RUN_LEASE_MILLIS, LifeStorageError,
    LifeStorageRepository, LifeStorageResult,
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

    async fn reap_expired_running_runs_in_tx(
        tx: &mut sqlx_core::transaction::Transaction<'_, Postgres>,
        principal_user_id: PrincipalUserId,
        now: TimestampMillis,
    ) -> LifeStorageResult<()> {
        query::<Postgres>(
            r#"
            UPDATE life_runs
            SET status = 'failed',
                finished_at = $2,
                error_text = COALESCE(error_text, 'run lease expired'),
                updated_at = $2
            WHERE principal_user_id = $1
              AND status = 'running'
              AND (lease_expires_at IS NULL OR lease_expires_at <= $2)
            "#,
        )
        .bind(principal_user_id.get())
        .bind(now.get())
        .execute(&mut **tx)
        .await
        .map_err(db_error)?;
        Ok(())
    }

    fn lease_expires_at(now: TimestampMillis) -> TimestampMillis {
        TimestampMillis::new(now.get() + LIFE_RUN_LEASE_MILLIS)
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

    /// Lists transcript turns in **ascending** chronological order (oldest first).
    ///
    /// When `cursor` is `None`, returns the most recent `limit` turns (the
    /// transcript tail) in ascending order. When `cursor` is `Some`, returns
    /// turns strictly **after** the cursor position in ascending order.
    ///
    /// Used by the life SSE handler for initial tail delivery and subsequent
    /// replay/live-delivery of new turns.
    pub async fn list_turns_ascending(
        &self,
        principal_user_id: PrincipalUserId,
        cursor: Option<&str>,
        limit: i64,
    ) -> LifeStorageResult<Vec<LifeTurn>> {
        if let Some(parsed) = parse_turn_cursor(cursor)? {
            let rows = query::<Postgres>(
                r#"
                SELECT *
                FROM life_turns
                WHERE principal_user_id = $1
                  AND (created_at > $2
                       OR (created_at = $2 AND turn_id > $3))
                ORDER BY created_at ASC, turn_id ASC
                LIMIT $4
                "#,
            )
            .bind(principal_user_id.get())
            .bind(parsed.created_at)
            .bind(parsed.turn_id)
            .bind(limit)
            .fetch_all(&self.pool)
            .await
            .map_err(db_error)?;

            return rows.into_iter().map(turn_from_row).collect();
        }

        // No cursor: fetch the most recent `limit` turns (DESC) then reverse
        // to ascending order. This avoids a subquery and gives the SSE handler
        // the transcript tail on initial connect.
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

        let mut turns: Vec<LifeTurn> = rows
            .into_iter()
            .map(turn_from_row)
            .collect::<LifeStorageResult<Vec<_>>>()?;
        turns.reverse();
        Ok(turns)
    }

    /// Lists run events in **ascending** chronological order (oldest first).
    ///
    /// When `cursor` is `None`, returns the most recent `limit` events (the
    /// activity tail) in ascending order. When `cursor` is `Some`, returns
    /// events strictly **after** the cursor position in ascending order.
    ///
    /// When `run_id` is `Some`, events are scoped to that run. When `None`,
    /// events are scoped to the principal (all runs).
    ///
    /// Used by the life SSE handler for initial tail delivery and subsequent
    /// replay/live-delivery of new events.
    pub async fn list_events_ascending(
        &self,
        principal_user_id: PrincipalUserId,
        run_id: Option<RunId>,
        cursor: Option<&str>,
        limit: i64,
    ) -> LifeStorageResult<Vec<LifeEvent>> {
        let parsed = parse_event_cursor(cursor)?;

        // Cursor provided: fetch strictly after the cursor position, ASC.
        if let Some(cur) = parsed {
            let rows = match run_id {
                Some(rid) => query::<Postgres>(
                    r#"
                    SELECT *
                    FROM life_events
                    WHERE run_id = $1
                      AND (created_at > $2
                           OR (created_at = $2 AND run_id > $3)
                           OR (created_at = $2 AND run_id = $3 AND seq > $4))
                    ORDER BY created_at ASC, run_id ASC, seq ASC
                    LIMIT $5
                    "#,
                )
                .bind(rid.as_uuid())
                .bind(cur.created_at)
                .bind(cur.run_id)
                .bind(cur.seq)
                .bind(limit)
                .fetch_all(&self.pool)
                .await
                .map_err(db_error)?,
                None => query::<Postgres>(
                    r#"
                    SELECT events.*
                    FROM life_events events
                    INNER JOIN life_runs runs ON runs.run_id = events.run_id
                    WHERE runs.principal_user_id = $1
                      AND (events.created_at > $2
                           OR (events.created_at = $2 AND events.run_id > $3)
                           OR (events.created_at = $2 AND events.run_id = $3 AND events.seq > $4))
                    ORDER BY events.created_at ASC, events.run_id ASC, events.seq ASC
                    LIMIT $5
                    "#,
                )
                .bind(principal_user_id.get())
                .bind(cur.created_at)
                .bind(cur.run_id)
                .bind(cur.seq)
                .bind(limit)
                .fetch_all(&self.pool)
                .await
                .map_err(db_error)?,
            };

            return rows.into_iter().map(event_from_row).collect();
        }

        // No cursor: fetch the most recent `limit` events (DESC) then reverse.
        let rows = match run_id {
            Some(rid) => query::<Postgres>(
                r#"
                SELECT *
                FROM life_events
                WHERE run_id = $1
                ORDER BY created_at DESC, run_id ASC, seq DESC
                LIMIT $2
                "#,
            )
            .bind(rid.as_uuid())
            .bind(limit)
            .fetch_all(&self.pool)
            .await
            .map_err(db_error)?,
            None => query::<Postgres>(
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
            .map_err(db_error)?,
        };

        let mut events: Vec<LifeEvent> = rows
            .into_iter()
            .map(event_from_row)
            .collect::<LifeStorageResult<Vec<_>>>()?;
        events.reverse();
        Ok(events)
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
                transport_id, provider_subject, principal_user_id, verified_at, created_at, updated_at
            )
            VALUES ($1, $2, $3, $4, $5, $6)
            ON CONFLICT (transport_id, provider_subject) DO UPDATE
            SET verified_at = EXCLUDED.verified_at,
                updated_at = EXCLUDED.updated_at
            WHERE life_identity_links.principal_user_id = EXCLUDED.principal_user_id
            RETURNING principal_user_id
            "#,
        )
        .bind(link.transport_id.as_str())
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
                transport_id: link.transport_id.clone(),
                provider_subject: link.provider_subject.clone(),
            });
        }
        Ok(())
    }

    async fn resolve_identity(
        &self,
        transport_id: &LifeTransportId,
        provider_subject: &ProviderSubject,
    ) -> LifeStorageResult<Option<PrincipalUserId>> {
        let row = query::<Postgres>(
            r#"
            SELECT principal_user_id
            FROM life_identity_links
            WHERE transport_id = $1 AND provider_subject = $2
            "#,
        )
        .bind(transport_id.as_str())
        .bind(provider_subject.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?;

        row.map(|row| PrincipalUserId::new(row.get::<i64, _>("principal_user_id")))
            .transpose()
            .map_err(Into::into)
    }

    async fn upsert_transport_binding(
        &self,
        binding: &LifeTransportBinding,
    ) -> LifeStorageResult<()> {
        query::<Postgres>(
            r#"
            INSERT INTO life_transport_bindings (
                binding_id, principal_user_id, transport_id, inbound_address,
                delivery_address, enabled, created_at, updated_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            ON CONFLICT (transport_id, inbound_address) DO UPDATE
            SET principal_user_id = EXCLUDED.principal_user_id,
                delivery_address = EXCLUDED.delivery_address,
                enabled = EXCLUDED.enabled,
                updated_at = EXCLUDED.updated_at
            "#,
        )
        .bind(binding.binding_id.as_uuid())
        .bind(binding.principal_user_id.get())
        .bind(binding.transport_id.as_str())
        .bind(&binding.inbound_address)
        .bind(&binding.delivery_address)
        .bind(binding.enabled)
        .bind(binding.created_at.get())
        .bind(binding.updated_at.get())
        .execute(&self.pool)
        .await
        .map_err(db_error)?;

        Ok(())
    }

    async fn resolve_transport_binding(
        &self,
        transport_id: &LifeTransportId,
        inbound_address: &Value,
    ) -> LifeStorageResult<Option<LifeTransportBinding>> {
        let row = query::<Postgres>(
            r#"
            SELECT *
            FROM life_transport_bindings
            WHERE transport_id = $1 AND inbound_address = $2 AND enabled = TRUE
            "#,
        )
        .bind(transport_id.as_str())
        .bind(inbound_address)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?;

        row.map(binding_from_row).transpose()
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
        .bind(turn.source_transport.as_str())
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

    async fn append_assistant_turn_and_enqueue_deliveries(
        &self,
        turn: &LifeTurn,
        now: TimestampMillis,
    ) -> LifeStorageResult<Vec<LifeDeliveryOutbox>> {
        if turn.role != LifeTurnRole::Assistant {
            return Err(LifeStorageError::InvalidOperation(
                "delivery outbox can only be enqueued for assistant turns".to_owned(),
            ));
        }

        let mut tx = self.pool.begin().await.map_err(db_error)?;
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
        .bind(turn.source_transport.as_str())
        .bind(&turn.source_ref)
        .bind(&turn.content)
        .bind(&turn.attachments)
        .bind(&turn.transport_metadata)
        .bind(redaction_state_as_str(turn.redaction_state))
        .bind(turn.created_at.get())
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;

        let binding_rows = query::<Postgres>(
            r#"
            SELECT *
            FROM life_transport_bindings
            WHERE principal_user_id = $1 AND enabled = TRUE
            ORDER BY transport_id ASC, binding_id ASC
            "#,
        )
        .bind(turn.principal_user_id.get())
        .fetch_all(&mut *tx)
        .await
        .map_err(db_error)?;

        let mut deliveries = Vec::with_capacity(binding_rows.len());
        for row in binding_rows {
            let binding = binding_from_row(row)?;
            let delivery_id = DeliveryId::new_v4();
            let inserted = query::<Postgres>(
                r#"
                INSERT INTO life_delivery_outbox (
                    delivery_id, turn_id, binding_id, principal_user_id, transport_id,
                    delivery_address, status, attempt_count, claimed_by, claimed_at,
                    claim_expires_at, next_attempt_at, last_error, created_at, updated_at
                )
                VALUES ($1, $2, $3, $4, $5, $6, 'queued', 0, NULL, NULL, NULL, $7, NULL, $7, $7)
                ON CONFLICT (turn_id, binding_id) DO NOTHING
                RETURNING *
                "#,
            )
            .bind(delivery_id.as_uuid())
            .bind(turn.turn_id.as_uuid())
            .bind(binding.binding_id.as_uuid())
            .bind(binding.principal_user_id.get())
            .bind(binding.transport_id.as_str())
            .bind(&binding.delivery_address)
            .bind(now.get())
            .fetch_optional(&mut *tx)
            .await
            .map_err(db_error)?;
            if let Some(row) = inserted {
                deliveries.push(delivery_from_row(row)?);
            }
        }

        tx.commit().await.map_err(db_error)?;
        Ok(deliveries)
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
        Self::reap_expired_running_runs_in_tx(&mut tx, principal_user_id, now).await?;

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
            WITH claimed AS (
                UPDATE life_inputs
                SET status = 'claimed', claimed_by = $3, claimed_at = $4, updated_at = $4
                WHERE input_id = $1 AND principal_user_id = $2 AND status = 'queued'
                RETURNING *
            )
            SELECT claimed.*, lt.content AS user_content
            FROM claimed
            JOIN life_turns lt ON lt.turn_id = claimed.turn_id
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
        let user_content: String = input_row.get("user_content");

        let lease_expires_at = Self::lease_expires_at(now);
        query::<Postgres>(
            r#"
            INSERT INTO life_runs (
                run_id, principal_user_id, status,
                started_at, finished_at, last_checkpoint_at, error_text,
                lease_owner, lease_expires_at, last_heartbeat_at,
                created_at, updated_at
            )
            VALUES ($1, $2, 'running', $3, NULL, NULL, NULL, $4, $5, $3, $3, $3)
            "#,
        )
        .bind(run_id.as_uuid())
        .bind(principal_user_id.get())
        .bind(now.get())
        .bind(worker_id)
        .bind(lease_expires_at.get())
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;

        tx.commit().await.map_err(db_error)?;

        Ok(Some(ClaimedLifeInputRun {
            input: input_from_row(input_row)?,
            run: LifeRun {
                run_id,
                principal_user_id,
                status: LifeRunStatus::Running,
                started_at: Some(now),
                finished_at: None,
                last_checkpoint_at: None,
                error_text: None,
                lease_owner: Some(worker_id.to_owned()),
                lease_expires_at: Some(lease_expires_at),
                last_heartbeat_at: Some(now),
                created_at: now,
                updated_at: now,
            },
            user_content,
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
            WHERE input_id = $1 AND status = 'claimed'
            "#,
        )
        .bind(input_id.as_uuid())
        .bind(now.get())
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(())
    }

    async fn claim_next_queued_input_and_start_run(
        &self,
        principal_user_id: PrincipalUserId,
        run_id: RunId,
        worker_id: &str,
        now: TimestampMillis,
    ) -> LifeStorageResult<Option<ClaimedLifeInputRun>> {
        let mut tx = self.pool.begin().await.map_err(db_error)?;
        Self::advisory_xact_lock_in_tx(&mut tx, principal_user_id).await?;
        Self::reap_expired_running_runs_in_tx(&mut tx, principal_user_id, now).await?;

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
            WITH next_input AS (
                SELECT input_id
                FROM life_inputs
                WHERE principal_user_id = $1 AND status = 'queued'
                ORDER BY created_at ASC, input_id ASC
                LIMIT 1
                FOR UPDATE SKIP LOCKED
            ), claimed AS (
                UPDATE life_inputs
                SET status = 'claimed', claimed_by = $2, claimed_at = $3, updated_at = $3
                WHERE input_id = (SELECT input_id FROM next_input)
                RETURNING *
            )
            SELECT claimed.*, lt.content AS user_content
            FROM claimed
            JOIN life_turns lt ON lt.turn_id = claimed.turn_id
            "#,
        )
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
        let user_content: String = input_row.get("user_content");

        let lease_expires_at = Self::lease_expires_at(now);
        query::<Postgres>(
            r#"
            INSERT INTO life_runs (
                run_id, principal_user_id, status,
                started_at, finished_at, last_checkpoint_at, error_text,
                lease_owner, lease_expires_at, last_heartbeat_at,
                created_at, updated_at
            )
            VALUES ($1, $2, 'running', $3, NULL, NULL, NULL, $4, $5, $3, $3, $3)
            "#,
        )
        .bind(run_id.as_uuid())
        .bind(principal_user_id.get())
        .bind(now.get())
        .bind(worker_id)
        .bind(lease_expires_at.get())
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;

        tx.commit().await.map_err(db_error)?;

        Ok(Some(ClaimedLifeInputRun {
            input: input_from_row(input_row)?,
            run: LifeRun {
                run_id,
                principal_user_id,
                status: LifeRunStatus::Running,
                started_at: Some(now),
                finished_at: None,
                last_checkpoint_at: None,
                error_text: None,
                lease_owner: Some(worker_id.to_owned()),
                lease_expires_at: Some(lease_expires_at),
                last_heartbeat_at: Some(now),
                created_at: now,
                updated_at: now,
            },
            user_content,
        }))
    }

    async fn heartbeat_run_lease(
        &self,
        run_id: RunId,
        worker_id: &str,
        now: TimestampMillis,
    ) -> LifeStorageResult<bool> {
        let lease_expires_at = Self::lease_expires_at(now);
        let result = query::<Postgres>(
            r#"
            UPDATE life_runs
            SET lease_expires_at = $3,
                last_heartbeat_at = $2,
                updated_at = $2
            WHERE run_id = $1
              AND status = 'running'
              AND lease_owner = $4
              AND (lease_expires_at IS NULL OR lease_expires_at > $2)
            "#,
        )
        .bind(run_id.as_uuid())
        .bind(now.get())
        .bind(lease_expires_at.get())
        .bind(worker_id)
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(result.rows_affected() == 1)
    }

    async fn find_active_run(
        &self,
        principal_user_id: PrincipalUserId,
    ) -> LifeStorageResult<Option<LifeRun>> {
        let row = query::<Postgres>(
            r#"
            SELECT *
            FROM life_runs
            WHERE principal_user_id = $1 AND status = 'running'
            LIMIT 1
            "#,
        )
        .bind(principal_user_id.get())
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?;
        row.map(run_from_row).transpose()
    }

    async fn link_turn_to_run(
        &self,
        turn_id: crate::domain::TurnId,
        run_id: RunId,
    ) -> LifeStorageResult<()> {
        query::<Postgres>(
            r#"
            UPDATE life_turns
            SET run_id = $2
            WHERE turn_id = $1
            "#,
        )
        .bind(turn_id.as_uuid())
        .bind(run_id.as_uuid())
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(())
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

    async fn claim_next_delivery(
        &self,
        transport_id: &LifeTransportId,
        worker_id: &str,
        now: TimestampMillis,
    ) -> LifeStorageResult<Option<ClaimedLifeDelivery>> {
        validate_delivery_worker_id(worker_id)?;
        let row = query::<Postgres>(
            r#"
            WITH candidate AS (
                SELECT delivery_id
                FROM life_delivery_outbox
                WHERE transport_id = $1
                  AND (
                    (status IN ('queued', 'failed') AND next_attempt_at <= $3)
                    OR (status = 'claimed' AND claim_expires_at <= $3)
                  )
                ORDER BY created_at ASC, delivery_id ASC
                FOR UPDATE SKIP LOCKED
                LIMIT 1
            )
            UPDATE life_delivery_outbox d
            SET status = 'claimed',
                attempt_count = d.attempt_count + 1,
                claimed_by = $2,
                claimed_at = $3,
                claim_expires_at = $4,
                updated_at = $3
            FROM candidate
            WHERE d.delivery_id = candidate.delivery_id
            RETURNING d.*, (SELECT content FROM life_turns WHERE turn_id = d.turn_id) AS content
            "#,
        )
        .bind(transport_id.as_str())
        .bind(worker_id.trim())
        .bind(now.get())
        .bind(now.get() + LIFE_DELIVERY_CLAIM_MILLIS)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?;

        row.map(claimed_delivery_from_row).transpose()
    }

    async fn mark_delivery_delivered(
        &self,
        delivery_id: DeliveryId,
        now: TimestampMillis,
    ) -> LifeStorageResult<()> {
        query::<Postgres>(
            r#"
            UPDATE life_delivery_outbox
            SET status = 'delivered',
                claimed_by = NULL,
                claimed_at = NULL,
                claim_expires_at = NULL,
                last_error = NULL,
                updated_at = $2
            WHERE delivery_id = $1 AND status = 'claimed'
            "#,
        )
        .bind(delivery_id.as_uuid())
        .bind(now.get())
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(())
    }

    async fn mark_delivery_failed(
        &self,
        delivery_id: DeliveryId,
        error_text: &str,
        next_attempt_at: TimestampMillis,
        now: TimestampMillis,
    ) -> LifeStorageResult<()> {
        query::<Postgres>(
            r#"
            UPDATE life_delivery_outbox
            SET status = 'failed',
                claimed_by = NULL,
                claimed_at = NULL,
                claim_expires_at = NULL,
                next_attempt_at = $3,
                last_error = $2,
                updated_at = $4
            WHERE delivery_id = $1 AND status = 'claimed'
            "#,
        )
        .bind(delivery_id.as_uuid())
        .bind(error_text)
        .bind(next_attempt_at.get())
        .bind(now.get())
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(())
    }

    async fn mark_delivery_dead(
        &self,
        delivery_id: DeliveryId,
        error_text: &str,
        now: TimestampMillis,
    ) -> LifeStorageResult<()> {
        query::<Postgres>(
            r#"
            UPDATE life_delivery_outbox
            SET status = 'dead',
                claimed_by = NULL,
                claimed_at = NULL,
                claim_expires_at = NULL,
                last_error = $2,
                updated_at = $3
            WHERE delivery_id = $1 AND status = 'claimed'
            "#,
        )
        .bind(delivery_id.as_uuid())
        .bind(error_text)
        .bind(now.get())
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(())
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

fn run_status_from_str(value: &str) -> LifeStorageResult<LifeRunStatus> {
    match value {
        "queued" => Ok(LifeRunStatus::Queued),
        "running" => Ok(LifeRunStatus::Running),
        "completed" => Ok(LifeRunStatus::Completed),
        "failed" => Ok(LifeRunStatus::Failed),
        "cancelled" => Ok(LifeRunStatus::Cancelled),
        other => unknown_enum("life_run_status", other),
    }
}

fn delivery_status_from_str(value: &str) -> LifeStorageResult<LifeDeliveryStatus> {
    match value {
        "queued" => Ok(LifeDeliveryStatus::Queued),
        "claimed" => Ok(LifeDeliveryStatus::Claimed),
        "delivered" => Ok(LifeDeliveryStatus::Delivered),
        "failed" => Ok(LifeDeliveryStatus::Failed),
        "dead" => Ok(LifeDeliveryStatus::Dead),
        other => unknown_enum("life_delivery_status", other),
    }
}

fn run_from_row(row: sqlx_postgres::PgRow) -> LifeStorageResult<LifeRun> {
    Ok(LifeRun {
        run_id: RunId::from_uuid(row.get("run_id")),
        principal_user_id: PrincipalUserId::new(row.get("principal_user_id"))?,
        status: run_status_from_str(row.get::<&str, _>("status"))?,
        started_at: row
            .get::<Option<i64>, _>("started_at")
            .map(TimestampMillis::new),
        finished_at: row
            .get::<Option<i64>, _>("finished_at")
            .map(TimestampMillis::new),
        last_checkpoint_at: row
            .get::<Option<i64>, _>("last_checkpoint_at")
            .map(TimestampMillis::new),
        error_text: row.get("error_text"),
        lease_owner: row.get("lease_owner"),
        lease_expires_at: row
            .get::<Option<i64>, _>("lease_expires_at")
            .map(TimestampMillis::new),
        last_heartbeat_at: row
            .get::<Option<i64>, _>("last_heartbeat_at")
            .map(TimestampMillis::new),
        created_at: TimestampMillis::new(row.get("created_at")),
        updated_at: TimestampMillis::new(row.get("updated_at")),
    })
}

fn unknown_enum<T>(type_name: &'static str, value: &str) -> LifeStorageResult<T> {
    Err(LifeStorageError::UnknownEnumValue {
        type_name,
        value: value.to_owned(),
    })
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

fn binding_from_row(row: sqlx_postgres::PgRow) -> LifeStorageResult<LifeTransportBinding> {
    Ok(LifeTransportBinding {
        binding_id: BindingId::from_uuid(row.get("binding_id")),
        principal_user_id: PrincipalUserId::new(row.get("principal_user_id"))?,
        transport_id: LifeTransportId::new(row.get::<&str, _>("transport_id"))?,
        inbound_address: row.get("inbound_address"),
        delivery_address: row.get("delivery_address"),
        enabled: row.get("enabled"),
        created_at: TimestampMillis::new(row.get("created_at")),
        updated_at: TimestampMillis::new(row.get("updated_at")),
    })
}

fn delivery_from_row(row: sqlx_postgres::PgRow) -> LifeStorageResult<LifeDeliveryOutbox> {
    Ok(LifeDeliveryOutbox {
        delivery_id: DeliveryId::from_uuid(row.get("delivery_id")),
        turn_id: TurnId::from_uuid(row.get("turn_id")),
        binding_id: BindingId::from_uuid(row.get("binding_id")),
        principal_user_id: PrincipalUserId::new(row.get("principal_user_id"))?,
        transport_id: LifeTransportId::new(row.get::<&str, _>("transport_id"))?,
        delivery_address: row.get("delivery_address"),
        status: delivery_status_from_str(row.get::<&str, _>("status"))?,
        attempt_count: row.get("attempt_count"),
        claimed_by: row.get("claimed_by"),
        claimed_at: row
            .get::<Option<i64>, _>("claimed_at")
            .map(TimestampMillis::new),
        claim_expires_at: row
            .get::<Option<i64>, _>("claim_expires_at")
            .map(TimestampMillis::new),
        next_attempt_at: TimestampMillis::new(row.get("next_attempt_at")),
        last_error: row.get("last_error"),
        created_at: TimestampMillis::new(row.get("created_at")),
        updated_at: TimestampMillis::new(row.get("updated_at")),
    })
}

fn claimed_delivery_from_row(row: sqlx_postgres::PgRow) -> LifeStorageResult<ClaimedLifeDelivery> {
    let content = row.get("content");
    Ok(ClaimedLifeDelivery {
        delivery: delivery_from_row(row)?,
        content,
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
        source_transport: LifeTransportId::new(row.get::<&str, _>("source_transport"))?,
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

pub fn encode_turn_cursor(created_at: i64, turn_id: TurnId) -> String {
    format!("{created_at}:{turn_id}")
}

pub fn encode_event_cursor(created_at: i64, run_id: RunId, seq: i64) -> String {
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

    use serde_json::json;
    use sqlx_postgres::PgPoolOptions;

    use crate::domain::{EventId, INTERNAL_TRANSPORT_ID};

    use super::*;

    static USER_COUNTER: AtomicI64 = AtomicI64::new(1);

    #[tokio::test]
    async fn sqlx_transport_bindings_resolve_open_enabled_addresses() {
        let Some(storage) = sqlx_test_storage().await else {
            return;
        };
        let principal_user_id = unique_principal_user_id();
        let now = TimestampMillis::new(1_700_000_020_000);
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

        let telegram = LifeTransportId::new("telegram").expect("telegram transport id");
        let inbound = json!({ "chat_id": "424242" });
        let binding = LifeTransportBinding {
            binding_id: BindingId::new_v4(),
            principal_user_id,
            transport_id: telegram.clone(),
            inbound_address: inbound.clone(),
            delivery_address: json!({ "chat_id": "424242" }),
            enabled: true,
            created_at: now,
            updated_at: now,
        };
        must(
            storage.upsert_transport_binding(&binding).await,
            "upsert telegram binding",
        );

        let resolved = must(
            storage.resolve_transport_binding(&telegram, &inbound).await,
            "resolve telegram binding",
        )
        .expect("enabled binding should resolve");
        assert_eq!(resolved.principal_user_id, principal_user_id);
        assert_eq!(resolved.transport_id.as_str(), "telegram");
        assert_eq!(resolved.inbound_address, inbound);

        let linux = LifeTransportId::new("linux").expect("future transport id");
        let linux_binding = LifeTransportBinding {
            binding_id: BindingId::new_v4(),
            principal_user_id,
            transport_id: linux.clone(),
            inbound_address: json!({ "instance_id": "desktop-1" }),
            delivery_address: json!({ "instance_id": "desktop-1" }),
            enabled: true,
            created_at: now,
            updated_at: now,
        };
        must(
            storage.upsert_transport_binding(&linux_binding).await,
            "upsert linux binding",
        );
        assert!(
            must(
                storage
                    .resolve_transport_binding(&linux, &json!({ "instance_id": "desktop-1" }))
                    .await,
                "resolve linux binding",
            )
            .is_some()
        );

        let disabled = LifeTransportBinding {
            enabled: false,
            updated_at: TimestampMillis::new(now.get() + 1),
            ..binding
        };
        must(
            storage.upsert_transport_binding(&disabled).await,
            "disable telegram binding",
        );
        assert!(
            must(
                storage.resolve_transport_binding(&telegram, &inbound).await,
                "resolve disabled telegram binding",
            )
            .is_none()
        );
    }

    #[tokio::test]
    async fn sqlx_delivery_outbox_enqueue_claim_retry_and_deliver_are_db_backed() {
        let Some(storage) = sqlx_test_storage().await else {
            return;
        };
        let principal_user_id = unique_principal_user_id();
        let now = TimestampMillis::new(1_700_000_030_000);
        must(
            storage
                .upsert_principal(&LifePrincipal {
                    principal_user_id,
                    profile_state: json!({}),
                    operating_profile: json!({}),
                    settings: json!({}),
                    schema_version: 1,
                    created_at: now,
                    updated_at: now,
                })
                .await,
            "upsert principal",
        );

        let telegram = LifeTransportId::new("telegram").expect("telegram transport");
        let linux = LifeTransportId::new("linux").expect("linux transport");
        for (transport_id, address) in [
            (telegram.clone(), json!({ "chat_id": "424242" })),
            (linux.clone(), json!({ "instance_id": "desktop-1" })),
        ] {
            must(
                storage
                    .upsert_transport_binding(&LifeTransportBinding {
                        binding_id: BindingId::new_v4(),
                        principal_user_id,
                        transport_id,
                        inbound_address: address.clone(),
                        delivery_address: address,
                        enabled: true,
                        created_at: now,
                        updated_at: now,
                    })
                    .await,
                "upsert binding",
            );
        }

        let user_turn = user_turn(principal_user_id, "not assistant", now);
        let invalid = storage
            .append_assistant_turn_and_enqueue_deliveries(&user_turn, now)
            .await
            .expect_err("user turn must not enqueue deliveries");
        assert!(matches!(invalid, LifeStorageError::InvalidOperation(_)));

        let assistant_turn = LifeTurn {
            turn_id: TurnId::new_v4(),
            principal_user_id,
            run_id: None,
            role: LifeTurnRole::Assistant,
            source_transport: LifeTransportId::new(INTERNAL_TRANSPORT_ID)
                .expect("internal transport"),
            source_ref: None,
            content: "assistant response".to_owned(),
            attachments: json!([]),
            transport_metadata: json!({}),
            redaction_state: RedactionState::Clean,
            created_at: now,
        };
        let deliveries = must(
            storage
                .append_assistant_turn_and_enqueue_deliveries(&assistant_turn, now)
                .await,
            "append assistant and enqueue deliveries",
        );
        assert_eq!(deliveries.len(), 2);
        assert!(deliveries.iter().all(|delivery| {
            delivery.turn_id == assistant_turn.turn_id
                && delivery.principal_user_id == principal_user_id
                && delivery.status == LifeDeliveryStatus::Queued
                && delivery.attempt_count == 0
                && delivery.claimed_by.is_none()
        }));

        let first_claim_time = TimestampMillis::new(now.get() + 1);
        let claimed = must(
            storage
                .claim_next_delivery(&telegram, "telegram-worker", first_claim_time)
                .await,
            "claim telegram delivery",
        )
        .expect("telegram delivery should claim");
        assert_eq!(claimed.content, "assistant response");
        assert_eq!(claimed.delivery.transport_id.as_str(), "telegram");
        assert_eq!(claimed.delivery.status, LifeDeliveryStatus::Claimed);
        assert_eq!(claimed.delivery.attempt_count, 1);
        assert_eq!(
            claimed.delivery.claimed_by.as_deref(),
            Some("telegram-worker")
        );
        assert_eq!(
            claimed.delivery.claim_expires_at,
            Some(TimestampMillis::new(
                first_claim_time.get() + LIFE_DELIVERY_CLAIM_MILLIS
            ))
        );

        assert!(
            must(
                storage
                    .claim_next_delivery(
                        &telegram,
                        "other-worker",
                        TimestampMillis::new(first_claim_time.get() + 1),
                    )
                    .await,
                "claim before claim expiry",
            )
            .is_none(),
            "non-expired claimed delivery is not double-claimed"
        );

        let reclaim_time =
            TimestampMillis::new(first_claim_time.get() + LIFE_DELIVERY_CLAIM_MILLIS + 1);
        let reclaimed = must(
            storage
                .claim_next_delivery(&telegram, "other-worker", reclaim_time)
                .await,
            "reclaim expired delivery",
        )
        .expect("expired claimed delivery should reclaim");
        assert_eq!(reclaimed.delivery.delivery_id, claimed.delivery.delivery_id);
        assert_eq!(reclaimed.delivery.attempt_count, 2);
        assert_eq!(
            reclaimed.delivery.claimed_by.as_deref(),
            Some("other-worker")
        );

        let retry_at = TimestampMillis::new(reclaim_time.get() + 10_000);
        must(
            storage
                .mark_delivery_failed(
                    reclaimed.delivery.delivery_id,
                    "temporary transport error",
                    retry_at,
                    reclaim_time,
                )
                .await,
            "mark failed",
        );
        assert!(
            must(
                storage
                    .claim_next_delivery(
                        &telegram,
                        "telegram-worker",
                        TimestampMillis::new(retry_at.get() - 1),
                    )
                    .await,
                "claim before retry_at",
            )
            .is_none(),
            "failed delivery is not claimable before next_attempt_at"
        );

        let retry_claim = must(
            storage
                .claim_next_delivery(&telegram, "telegram-worker", retry_at)
                .await,
            "claim retry",
        )
        .expect("failed delivery should retry at next_attempt_at");
        assert_eq!(retry_claim.delivery.attempt_count, 3);
        must(
            storage
                .mark_delivery_delivered(retry_claim.delivery.delivery_id, retry_at)
                .await,
            "mark delivered",
        );

        let delivered_row = must(
            query::<Postgres>(
                "SELECT status, last_error FROM life_delivery_outbox WHERE delivery_id = $1",
            )
            .bind(retry_claim.delivery.delivery_id.as_uuid())
            .fetch_one(storage.pool())
            .await,
            "load delivered row",
        );
        assert_eq!(delivered_row.get::<String, _>("status"), "delivered");
        assert!(
            delivered_row
                .get::<Option<String>, _>("last_error")
                .is_none()
        );

        let linux_claim = must(
            storage
                .claim_next_delivery(&linux, "linux-worker", TimestampMillis::new(now.get() + 2))
                .await,
            "claim linux delivery",
        )
        .expect("future linux transport delivery should claim");
        assert_eq!(linux_claim.delivery.transport_id.as_str(), "linux");
        must(
            storage
                .mark_delivery_dead(
                    linux_claim.delivery.delivery_id,
                    "permanent adapter error",
                    TimestampMillis::new(now.get() + 3),
                )
                .await,
            "mark linux dead",
        );
    }

    #[tokio::test]
    async fn sqlx_life_worker_claim_start_complete_and_claim_next_are_db_backed() {
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
        assert_eq!(claimed.user_content, "first");

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
        let blocked_next = must(
            storage
                .claim_next_queued_input_and_start_run(
                    principal_user_id,
                    RunId::new_v4(),
                    "worker-sqlx",
                    TimestampMillis::new(now.get() + 4),
                )
                .await,
            "claim next while first run is still running",
        );
        assert!(blocked_next.is_none(), "running run blocks follow-up claim");

        must(
            storage
                .mark_input_consumed(follow_up.input_id, TimestampMillis::new(now.get() + 4))
                .await,
            "queued follow-up cannot be consumed by mark_input_consumed",
        );

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

        let follow_up_run_id = RunId::new_v4();
        let claimed_follow_up = must(
            storage
                .claim_next_queued_input_and_start_run(
                    principal_user_id,
                    follow_up_run_id,
                    "worker-sqlx",
                    TimestampMillis::new(now.get() + 7),
                )
                .await,
            "claim follow-up after first run completed",
        )
        .expect("follow-up should remain queued until its own run");
        assert_eq!(claimed_follow_up.input.input_id, follow_up.input_id);
        assert_eq!(claimed_follow_up.input.status, LifeInputStatus::Claimed);
        assert_eq!(claimed_follow_up.run.run_id, follow_up_run_id);
        assert_eq!(claimed_follow_up.user_content, "follow-up");

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

    #[tokio::test]
    async fn sqlx_life_run_lease_heartbeat_and_expiry_unblock_claims() {
        let Some(storage) = sqlx_test_storage().await else {
            return;
        };
        let principal_user_id = unique_principal_user_id();
        let now = TimestampMillis::new(1_700_000_050_000);
        must(
            storage
                .upsert_principal(&LifePrincipal {
                    principal_user_id,
                    profile_state: json!({}),
                    operating_profile: json!({}),
                    settings: json!({}),
                    schema_version: 1,
                    created_at: now,
                    updated_at: now,
                })
                .await,
            "upsert principal",
        );

        let first_turn = user_turn(principal_user_id, "first", now);
        must(storage.append_turn(&first_turn).await, "append first turn");
        let first_input = queued_input(principal_user_id, first_turn.turn_id, now);
        must(storage.enqueue_input(&first_input).await, "enqueue first");

        let first_run_id = RunId::new_v4();
        let claimed = must(
            storage
                .claim_input_and_start_run(
                    principal_user_id,
                    first_input.input_id,
                    first_run_id,
                    "worker-a",
                    TimestampMillis::new(now.get() + 1),
                )
                .await,
            "claim first input",
        )
        .expect("first input should claim");
        assert_eq!(claimed.run.lease_owner.as_deref(), Some("worker-a"));
        assert_eq!(
            claimed.run.last_heartbeat_at,
            Some(TimestampMillis::new(now.get() + 1))
        );
        assert_eq!(
            claimed.run.lease_expires_at,
            Some(TimestampMillis::new(now.get() + 1 + LIFE_RUN_LEASE_MILLIS))
        );

        let heartbeat_at = TimestampMillis::new(now.get() + 2);
        assert!(must(
            storage
                .heartbeat_run_lease(first_run_id, "worker-a", heartbeat_at)
                .await,
            "heartbeat owned run",
        ));
        assert!(!must(
            storage
                .heartbeat_run_lease(
                    first_run_id,
                    "other-worker",
                    TimestampMillis::new(now.get() + 3)
                )
                .await,
            "heartbeat from wrong owner",
        ));

        let second_turn = user_turn(
            principal_user_id,
            "second",
            TimestampMillis::new(now.get() + 4),
        );
        must(
            storage.append_turn(&second_turn).await,
            "append second turn",
        );
        let second_input = queued_input(
            principal_user_id,
            second_turn.turn_id,
            TimestampMillis::new(now.get() + 4),
        );
        must(storage.enqueue_input(&second_input).await, "enqueue second");

        let before_expiry = TimestampMillis::new(heartbeat_at.get() + LIFE_RUN_LEASE_MILLIS - 1);
        assert!(
            must(
                storage
                    .claim_next_queued_input_and_start_run(
                        principal_user_id,
                        RunId::new_v4(),
                        "worker-b",
                        before_expiry,
                    )
                    .await,
                "claim before lease expiry",
            )
            .is_none(),
            "non-expired lease still blocks duplicate running run"
        );

        let after_expiry = TimestampMillis::new(heartbeat_at.get() + LIFE_RUN_LEASE_MILLIS + 1);
        let second_run_id = RunId::new_v4();
        let claimed_second = must(
            storage
                .claim_next_queued_input_and_start_run(
                    principal_user_id,
                    second_run_id,
                    "worker-b",
                    after_expiry,
                )
                .await,
            "claim after lease expiry",
        )
        .expect("expired first run should be reaped and second input claimed");
        assert_eq!(claimed_second.input.input_id, second_input.input_id);
        assert_eq!(claimed_second.run.run_id, second_run_id);
        assert_eq!(claimed_second.run.lease_owner.as_deref(), Some("worker-b"));

        let first_run_row = must(
            query::<Postgres>(
                "SELECT status, finished_at, error_text FROM life_runs WHERE run_id = $1",
            )
            .bind(first_run_id.as_uuid())
            .fetch_one(storage.pool())
            .await,
            "load reaped first run",
        );
        assert_eq!(first_run_row.get::<String, _>("status"), "failed");
        assert_eq!(
            first_run_row.get::<i64, _>("finished_at"),
            after_expiry.get()
        );
        assert_eq!(
            first_run_row.get::<String, _>("error_text"),
            "run lease expired"
        );
    }

    #[tokio::test]
    async fn sqlx_life_find_active_run_and_link_turn_to_run() {
        let Some(storage) = sqlx_test_storage().await else {
            return;
        };
        let principal_user_id = unique_principal_user_id();
        let now = TimestampMillis::new(1_700_000_100_000);
        must(
            storage
                .upsert_principal(&LifePrincipal {
                    principal_user_id,
                    profile_state: json!({}),
                    operating_profile: json!({}),
                    settings: json!({}),
                    schema_version: 1,
                    created_at: now,
                    updated_at: now,
                })
                .await,
            "upsert principal",
        );

        // No active run before claiming.
        let no_run = must(
            storage.find_active_run(principal_user_id).await,
            "find active run before claim",
        );
        assert!(no_run.is_none());

        // Create a user turn and queue an input.
        let turn = user_turn(principal_user_id, "hello", now);
        must(storage.append_turn(&turn).await, "append turn");
        let input = queued_input(principal_user_id, turn.turn_id, now);
        must(storage.enqueue_input(&input).await, "enqueue input");

        // Claim the input and start a run.
        let run_id = RunId::new_v4();
        let _claimed = must(
            storage
                .claim_input_and_start_run(
                    principal_user_id,
                    input.input_id,
                    run_id,
                    "worker-sqlx",
                    TimestampMillis::new(now.get() + 1),
                )
                .await,
            "claim input and start run",
        )
        .expect("input should be claimed");
        assert_eq!(_claimed.user_content, "hello");

        // find_active_run should now return the running run.
        let active = must(
            storage.find_active_run(principal_user_id).await,
            "find active run after claim",
        )
        .expect("active run should exist");
        assert_eq!(active.run_id, run_id);
        assert_eq!(active.status, LifeRunStatus::Running);

        // life_turns.run_id should be NULL before linking.
        let turn_row_before = must(
            query::<Postgres>("SELECT run_id FROM life_turns WHERE turn_id = $1")
                .bind(turn.turn_id.as_uuid())
                .fetch_one(storage.pool())
                .await,
            "load turn before link",
        );
        assert!(turn_row_before.get::<Option<Uuid>, _>("run_id").is_none());

        // Link the originating turn to the run.
        must(
            storage.link_turn_to_run(turn.turn_id, run_id).await,
            "link turn to run",
        );

        // life_turns.run_id should now be set.
        let turn_row_after = must(
            query::<Postgres>("SELECT run_id FROM life_turns WHERE turn_id = $1")
                .bind(turn.turn_id.as_uuid())
                .fetch_one(storage.pool())
                .await,
            "load turn after link",
        );
        assert_eq!(
            turn_row_after.get::<Option<Uuid>, _>("run_id"),
            Some(run_id.as_uuid())
        );

        // Queue a follow-up input while the first run is active.
        let follow_up_turn = user_turn(
            principal_user_id,
            "follow-up",
            TimestampMillis::new(now.get() + 2),
        );
        must(
            storage.append_turn(&follow_up_turn).await,
            "append follow-up turn",
        );
        let follow_up_input = queued_input(
            principal_user_id,
            follow_up_turn.turn_id,
            TimestampMillis::new(now.get() + 2),
        );
        must(
            storage.enqueue_input(&follow_up_input).await,
            "enqueue follow-up",
        );
        let blocked_follow_up = must(
            storage
                .claim_next_queued_input_and_start_run(
                    principal_user_id,
                    RunId::new_v4(),
                    "worker-sqlx",
                    TimestampMillis::new(now.get() + 3),
                )
                .await,
            "claim follow-up while first run is active",
        );
        assert!(blocked_follow_up.is_none());

        // Complete the run; find_active_run should return None.
        must(
            storage
                .complete_run(
                    run_id,
                    TimestampMillis::new(now.get() + 4),
                    TimestampMillis::new(now.get() + 4),
                )
                .await,
            "complete run",
        );
        let no_active = must(
            storage.find_active_run(principal_user_id).await,
            "find active run after complete",
        );
        assert!(no_active.is_none());

        let follow_up_run_id = RunId::new_v4();
        let claimed_follow_up = must(
            storage
                .claim_next_queued_input_and_start_run(
                    principal_user_id,
                    follow_up_run_id,
                    "worker-sqlx",
                    TimestampMillis::new(now.get() + 5),
                )
                .await,
            "claim follow-up after first run complete",
        )
        .expect("follow-up should claim after active run completes");
        assert_eq!(claimed_follow_up.input.input_id, follow_up_input.input_id);
        assert_eq!(claimed_follow_up.user_content, "follow-up");
        must(
            storage
                .link_turn_to_run(follow_up_turn.turn_id, follow_up_run_id)
                .await,
            "link follow-up turn to its own run",
        );
        let follow_up_row = must(
            query::<Postgres>("SELECT run_id FROM life_turns WHERE turn_id = $1")
                .bind(follow_up_turn.turn_id.as_uuid())
                .fetch_one(storage.pool())
                .await,
            "load follow-up turn after link",
        );
        assert_eq!(
            follow_up_row.get::<Option<Uuid>, _>("run_id"),
            Some(follow_up_run_id.as_uuid())
        );
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

        // Append 5 turns with strictly increasing timestamps.
        let mut turns = Vec::new();
        for i in 0..5 {
            let turn = LifeTurn {
                turn_id: TurnId::new_v4(),
                principal_user_id,
                run_id: None,
                role: LifeTurnRole::User,
                source_transport: LifeTransportId::new(INTERNAL_TRANSPORT_ID)
                    .expect("internal transport id"),
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
        must(
            query::<Postgres>(
                r#"
                INSERT INTO life_runs (
                    run_id, principal_user_id, status,
                    started_at, finished_at, last_checkpoint_at, error_text, created_at, updated_at
                )
                VALUES ($1, $2, 'completed', $3, $3, $3, NULL, $3, $3)
                "#,
            )
            .bind(run_id.as_uuid())
            .bind(principal_user_id.get())
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
            source_transport: LifeTransportId::new(INTERNAL_TRANSPORT_ID)
                .expect("internal transport id"),
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

    fn must<T, E: std::fmt::Display>(result: Result<T, E>, context: &str) -> T {
        match result {
            Ok(value) => value,
            Err(error) => panic!("{context}: {error}"),
        }
    }
}
