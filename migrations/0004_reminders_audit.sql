-- SQLx/Postgres reminder queue and append-only audit storage.

CREATE TABLE reminder_jobs (
    user_id BIGINT NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    reminder_id TEXT NOT NULL,
    context_key TEXT NOT NULL,
    flow_id TEXT NOT NULL,
    chat_id BIGINT NOT NULL,
    thread_id BIGINT,
    thread_kind TEXT NOT NULL CHECK (thread_kind IN ('dm', 'forum', 'none')),
    task_prompt TEXT NOT NULL,
    schedule_kind TEXT NOT NULL CHECK (schedule_kind IN ('once', 'interval', 'cron')),
    status TEXT NOT NULL CHECK (status IN ('scheduled', 'paused', 'completed', 'cancelled', 'failed')),
    next_run_at BIGINT NOT NULL,
    interval_secs BIGINT CHECK (interval_secs IS NULL OR interval_secs >= 0),
    cron_expression TEXT,
    timezone TEXT,
    lease_until BIGINT,
    last_run_at BIGINT,
    last_error TEXT,
    run_count BIGINT NOT NULL DEFAULT 0 CHECK (run_count >= 0),
    version BIGINT NOT NULL CHECK (version >= 1),
    schema_version INTEGER NOT NULL DEFAULT 2,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    PRIMARY KEY (user_id, reminder_id)
);

CREATE INDEX reminder_jobs_context_status_idx
    ON reminder_jobs (user_id, context_key, status, next_run_at DESC, created_at DESC);

CREATE INDEX reminder_jobs_due_idx
    ON reminder_jobs (user_id, status, next_run_at ASC, created_at ASC)
    WHERE status = 'scheduled';

CREATE INDEX reminder_jobs_lease_idx
    ON reminder_jobs (user_id, status, lease_until)
    WHERE status = 'scheduled' AND lease_until IS NOT NULL;

CREATE TABLE audit_stream_versions (
    user_id BIGINT PRIMARY KEY REFERENCES users(user_id) ON DELETE CASCADE,
    next_version BIGINT NOT NULL DEFAULT 1 CHECK (next_version >= 1),
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL
);

CREATE TABLE audit_events (
    user_id BIGINT NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    version BIGINT NOT NULL CHECK (version >= 1),
    event_id TEXT NOT NULL UNIQUE,
    topic_id TEXT,
    agent_id TEXT,
    action TEXT NOT NULL,
    payload JSONB NOT NULL,
    schema_version INTEGER NOT NULL DEFAULT 1,
    created_at BIGINT NOT NULL,
    PRIMARY KEY (user_id, version)
);

CREATE INDEX audit_events_user_version_desc_idx
    ON audit_events (user_id, version DESC);

CREATE INDEX audit_events_user_created_desc_idx
    ON audit_events (user_id, created_at DESC);

CREATE INDEX audit_events_topic_version_idx
    ON audit_events (user_id, topic_id, version DESC)
    WHERE topic_id IS NOT NULL;
