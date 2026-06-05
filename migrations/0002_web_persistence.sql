-- SQLx/Postgres web console persistence.

CREATE TABLE users (
    user_id BIGINT PRIMARY KEY,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE web_users (
    user_id BIGINT PRIMARY KEY REFERENCES users(user_id) ON DELETE CASCADE,
    login TEXT NOT NULL,
    normalized_login TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    role TEXT NOT NULL CHECK (role IN ('user', 'admin')),
    status TEXT NOT NULL CHECK (status IN ('active', 'disabled')),
    default_model_selection JSONB,
    default_agent_profile_id TEXT,
    default_effort TEXT CHECK (default_effort IS NULL OR default_effort IN ('standard', 'extended', 'heavy')),
    last_login_at TIMESTAMPTZ,
    schema_version INTEGER NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE login_identities (
    identity_id UUID PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    provider TEXT NOT NULL,
    provider_subject TEXT NOT NULL,
    normalized_login TEXT,
    password_hash TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (provider, provider_subject)
);

CREATE UNIQUE INDEX login_identities_password_login_uq
    ON login_identities (normalized_login)
    WHERE provider = 'password' AND normalized_login IS NOT NULL;

CREATE TABLE auth_sessions (
    session_token_hash TEXT PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    csrf_token TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    last_seen_at TIMESTAMPTZ NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    revoked_at TIMESTAMPTZ,
    schema_version INTEGER NOT NULL DEFAULT 1
);

CREATE INDEX auth_sessions_user_active_idx
    ON auth_sessions (user_id, expires_at)
    WHERE revoked_at IS NULL;

CREATE INDEX auth_sessions_expiry_idx
    ON auth_sessions (expires_at)
    WHERE revoked_at IS NULL;

CREATE TABLE web_sessions (
    user_id BIGINT NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    session_id TEXT NOT NULL,
    title TEXT NOT NULL,
    context_key TEXT NOT NULL,
    context_keys TEXT[] NOT NULL DEFAULT '{}',
    agent_flow_id TEXT NOT NULL,
    model_selection JSONB,
    agent_profile_id TEXT,
    active_task_id TEXT,
    last_task_status TEXT CHECK (last_task_status IS NULL OR last_task_status IN (
        'queued', 'running', 'waiting_for_user_input', 'completed', 'failed', 'cancelled', 'interrupted'
    )),
    last_preview TEXT,
    manually_renamed BOOLEAN NOT NULL DEFAULT FALSE,
    schema_version INTEGER NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (user_id, session_id)
);

CREATE INDEX web_sessions_user_updated_idx
    ON web_sessions (user_id, updated_at DESC);

CREATE TABLE web_tasks (
    user_id BIGINT NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    session_id TEXT NOT NULL,
    task_id TEXT NOT NULL,
    version_group_id TEXT NOT NULL,
    version_index INTEGER NOT NULL DEFAULT 1,
    parent_task_id TEXT,
    status TEXT NOT NULL CHECK (status IN (
        'queued', 'running', 'waiting_for_user_input', 'completed', 'failed', 'cancelled', 'interrupted'
    )),
    input_markdown TEXT NOT NULL,
    attachments JSONB NOT NULL DEFAULT '[]'::JSONB,
    input_edited_at TIMESTAMPTZ,
    final_response_markdown TEXT,
    error_message TEXT,
    pending_user_input JSONB,
    last_event_seq BIGINT NOT NULL DEFAULT 0,
    schema_version INTEGER NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL,
    started_at TIMESTAMPTZ,
    updated_at TIMESTAMPTZ NOT NULL,
    finished_at TIMESTAMPTZ,
    PRIMARY KEY (user_id, session_id, task_id),
    FOREIGN KEY (user_id, session_id)
        REFERENCES web_sessions(user_id, session_id)
        ON DELETE CASCADE
);

CREATE INDEX web_tasks_session_created_idx
    ON web_tasks (user_id, session_id, created_at ASC);

CREATE INDEX web_tasks_unfinished_idx
    ON web_tasks (status, updated_at)
    WHERE status IN ('queued', 'running', 'waiting_for_user_input');

CREATE INDEX web_tasks_version_lineage_idx
    ON web_tasks (user_id, session_id, version_group_id, version_index);

CREATE TABLE web_task_events (
    id BIGSERIAL PRIMARY KEY,
    user_id BIGINT NOT NULL,
    session_id TEXT NOT NULL,
    task_id TEXT NOT NULL,
    seq BIGINT NOT NULL,
    kind TEXT NOT NULL,
    summary TEXT NOT NULL,
    payload JSONB NOT NULL DEFAULT '{}'::JSONB,
    redacted BOOLEAN NOT NULL DEFAULT FALSE,
    truncated BOOLEAN NOT NULL DEFAULT FALSE,
    schema_version INTEGER NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL,
    retention_expires_at TIMESTAMPTZ,
    FOREIGN KEY (user_id, session_id, task_id)
        REFERENCES web_tasks(user_id, session_id, task_id)
        ON DELETE CASCADE,
    UNIQUE (user_id, session_id, task_id, seq)
);

CREATE INDEX web_task_events_page_idx
    ON web_task_events (user_id, session_id, task_id, seq ASC);

CREATE INDEX web_task_events_retention_idx
    ON web_task_events (retention_expires_at)
    WHERE retention_expires_at IS NOT NULL;

CREATE TABLE web_task_progress (
    user_id BIGINT NOT NULL,
    session_id TEXT NOT NULL,
    task_id TEXT NOT NULL,
    current_iteration INTEGER NOT NULL,
    max_iterations INTEGER NOT NULL,
    is_finished BOOLEAN NOT NULL,
    error TEXT,
    current_thought TEXT,
    progress_payload JSONB NOT NULL DEFAULT '{}'::JSONB,
    updated_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (user_id, session_id, task_id),
    FOREIGN KEY (user_id, session_id, task_id)
        REFERENCES web_tasks(user_id, session_id, task_id)
        ON DELETE CASCADE
);

CREATE TABLE web_task_files (
    user_id BIGINT NOT NULL,
    session_id TEXT NOT NULL,
    task_id TEXT NOT NULL,
    file_id TEXT NOT NULL,
    file_name TEXT NOT NULL,
    content_type TEXT NOT NULL,
    size_bytes BIGINT NOT NULL CHECK (size_bytes >= 0),
    sha256 TEXT,
    delivery_kind TEXT NOT NULL,
    storage_mode TEXT NOT NULL DEFAULT 'postgres_bytea' CHECK (storage_mode IN ('postgres_bytea')),
    schema_version INTEGER NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL,
    retention_expires_at TIMESTAMPTZ,
    PRIMARY KEY (user_id, session_id, task_id, file_id),
    FOREIGN KEY (user_id, session_id, task_id)
        REFERENCES web_tasks(user_id, session_id, task_id)
        ON DELETE CASCADE
);

CREATE TABLE web_task_file_blobs (
    user_id BIGINT NOT NULL,
    session_id TEXT NOT NULL,
    task_id TEXT NOT NULL,
    file_id TEXT NOT NULL,
    content BYTEA NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (user_id, session_id, task_id, file_id),
    FOREIGN KEY (user_id, session_id, task_id, file_id)
        REFERENCES web_task_files(user_id, session_id, task_id, file_id)
        ON DELETE CASCADE
);

CREATE INDEX web_task_files_retention_idx
    ON web_task_files (retention_expires_at)
    WHERE retention_expires_at IS NOT NULL;
