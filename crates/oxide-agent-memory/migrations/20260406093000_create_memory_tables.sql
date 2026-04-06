CREATE TABLE IF NOT EXISTS memory_threads (
    thread_id TEXT PRIMARY KEY,
    user_id BIGINT NOT NULL,
    context_key TEXT NOT NULL,
    title TEXT NOT NULL,
    short_summary TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    last_activity_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX IF NOT EXISTS memory_threads_context_user_activity_idx
    ON memory_threads (context_key, user_id, last_activity_at DESC);

CREATE TABLE IF NOT EXISTS memory_episodes (
    episode_id TEXT PRIMARY KEY,
    thread_id TEXT NOT NULL REFERENCES memory_threads(thread_id) ON DELETE CASCADE,
    context_key TEXT NOT NULL,
    goal TEXT NOT NULL,
    summary TEXT NOT NULL,
    outcome TEXT NOT NULL CHECK (outcome IN ('success', 'partial', 'failure', 'cancelled')),
    tools_used TEXT[] NOT NULL DEFAULT ARRAY[]::TEXT[],
    artifacts JSONB NOT NULL DEFAULT '[]'::JSONB,
    failures TEXT[] NOT NULL DEFAULT ARRAY[]::TEXT[],
    importance REAL NOT NULL CHECK (importance >= 0.0 AND importance <= 1.0),
    created_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX IF NOT EXISTS memory_episodes_thread_created_idx
    ON memory_episodes (thread_id, created_at DESC);

CREATE INDEX IF NOT EXISTS memory_episodes_context_created_idx
    ON memory_episodes (context_key, created_at DESC);

CREATE TABLE IF NOT EXISTS memory_records (
    memory_id TEXT PRIMARY KEY,
    context_key TEXT NOT NULL,
    source_episode_id TEXT NULL REFERENCES memory_episodes(episode_id) ON DELETE SET NULL,
    memory_type TEXT NOT NULL CHECK (memory_type IN ('fact', 'preference', 'procedure', 'decision', 'constraint')),
    title TEXT NOT NULL,
    content TEXT NOT NULL,
    short_description TEXT NOT NULL,
    importance REAL NOT NULL CHECK (importance >= 0.0 AND importance <= 1.0),
    confidence REAL NOT NULL CHECK (confidence >= 0.0 AND confidence <= 1.0),
    source TEXT NULL,
    reason TEXT NULL,
    tags TEXT[] NOT NULL DEFAULT ARRAY[]::TEXT[],
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX IF NOT EXISTS memory_records_context_updated_idx
    ON memory_records (context_key, updated_at DESC);

CREATE INDEX IF NOT EXISTS memory_records_context_type_updated_idx
    ON memory_records (context_key, memory_type, updated_at DESC);

CREATE TABLE IF NOT EXISTS memory_session_state (
    session_id TEXT PRIMARY KEY,
    context_key TEXT NOT NULL,
    hot_token_estimate BIGINT NOT NULL CHECK (hot_token_estimate >= 0),
    last_compacted_at TIMESTAMPTZ NULL,
    last_finalized_at TIMESTAMPTZ NULL,
    cleanup_status TEXT NOT NULL CHECK (cleanup_status IN ('active', 'idle', 'cleaning', 'finalized')),
    pending_episode_id TEXT NULL,
    updated_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX IF NOT EXISTS memory_session_state_context_updated_idx
    ON memory_session_state (context_key, updated_at DESC);
