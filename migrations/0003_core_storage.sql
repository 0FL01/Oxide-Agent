-- SQLx/Postgres core durable storage.

CREATE TABLE user_configs (
    user_id BIGINT PRIMARY KEY REFERENCES users(user_id) ON DELETE CASCADE,
    state TEXT,
    version BIGINT NOT NULL DEFAULT 1 CHECK (version >= 1),
    schema_version INTEGER NOT NULL DEFAULT 1,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL
);

CREATE TABLE user_contexts (
    user_id BIGINT NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    context_key TEXT NOT NULL,
    state TEXT,
    current_agent_flow_id TEXT,
    chat_id BIGINT,
    thread_id BIGINT,
    forum_topic_name TEXT,
    forum_topic_icon_color BIGINT CHECK (
        forum_topic_icon_color IS NULL
        OR (forum_topic_icon_color >= 0 AND forum_topic_icon_color <= 4294967295)
    ),
    forum_topic_icon_custom_emoji_id TEXT,
    forum_topic_closed BOOLEAN NOT NULL DEFAULT FALSE,
    version BIGINT NOT NULL DEFAULT 1 CHECK (version >= 1),
    schema_version INTEGER NOT NULL DEFAULT 1,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    PRIMARY KEY (user_id, context_key)
);

CREATE INDEX user_contexts_user_updated_idx
    ON user_contexts (user_id, updated_at DESC);

CREATE TABLE agent_memory_snapshots (
    user_id BIGINT NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    context_key TEXT NOT NULL DEFAULT '',
    flow_id TEXT NOT NULL DEFAULT '',
    memory JSONB NOT NULL,
    schema_version INTEGER NOT NULL DEFAULT 1,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    PRIMARY KEY (user_id, context_key, flow_id)
);

CREATE INDEX agent_memory_context_idx
    ON agent_memory_snapshots (user_id, context_key, updated_at DESC);

CREATE TABLE agent_flows (
    user_id BIGINT NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    context_key TEXT NOT NULL,
    flow_id TEXT NOT NULL,
    schema_version INTEGER NOT NULL DEFAULT 1,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    PRIMARY KEY (user_id, context_key, flow_id)
);

CREATE INDEX agent_flows_context_updated_idx
    ON agent_flows (user_id, context_key, updated_at DESC);

CREATE TABLE agent_profiles (
    user_id BIGINT NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    agent_id TEXT NOT NULL,
    profile JSONB NOT NULL,
    version BIGINT NOT NULL CHECK (version >= 1),
    schema_version INTEGER NOT NULL DEFAULT 1,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    PRIMARY KEY (user_id, agent_id)
);

CREATE INDEX agent_profiles_user_agent_idx
    ON agent_profiles (user_id, agent_id ASC);

CREATE TABLE topic_contexts (
    user_id BIGINT NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    topic_id TEXT NOT NULL,
    context TEXT NOT NULL,
    version BIGINT NOT NULL CHECK (version >= 1),
    schema_version INTEGER NOT NULL DEFAULT 1,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    PRIMARY KEY (user_id, topic_id)
);

CREATE TABLE topic_agents_md (
    user_id BIGINT NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    topic_id TEXT NOT NULL,
    agents_md TEXT NOT NULL,
    version BIGINT NOT NULL CHECK (version >= 1),
    schema_version INTEGER NOT NULL DEFAULT 1,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    PRIMARY KEY (user_id, topic_id)
);

CREATE TABLE topic_infra_configs (
    user_id BIGINT NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    topic_id TEXT NOT NULL,
    target_name TEXT NOT NULL,
    host TEXT NOT NULL,
    port INTEGER NOT NULL CHECK (port >= 0 AND port <= 65535),
    remote_user TEXT NOT NULL,
    auth_mode TEXT NOT NULL CHECK (auth_mode IN ('none', 'password', 'private_key')),
    secret_ref TEXT,
    sudo_secret_ref TEXT,
    environment TEXT,
    tags TEXT[] NOT NULL DEFAULT '{}',
    allowed_tool_modes TEXT[] NOT NULL DEFAULT '{}',
    approval_required_modes TEXT[] NOT NULL DEFAULT '{}',
    version BIGINT NOT NULL CHECK (version >= 1),
    schema_version INTEGER NOT NULL DEFAULT 1,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    PRIMARY KEY (user_id, topic_id)
);

CREATE TABLE topic_bindings (
    user_id BIGINT NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    topic_id TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    binding_kind TEXT NOT NULL CHECK (binding_kind IN ('manual', 'runtime')),
    chat_id BIGINT,
    thread_id BIGINT,
    expires_at BIGINT,
    last_activity_at BIGINT,
    version BIGINT NOT NULL CHECK (version >= 1),
    schema_version INTEGER NOT NULL DEFAULT 1,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    PRIMARY KEY (user_id, topic_id)
);

CREATE INDEX topic_bindings_transport_idx
    ON topic_bindings (user_id, chat_id, thread_id)
    WHERE chat_id IS NOT NULL OR thread_id IS NOT NULL;

CREATE INDEX topic_bindings_expiry_idx
    ON topic_bindings (expires_at)
    WHERE expires_at IS NOT NULL;

CREATE TABLE private_secrets (
    user_id BIGINT NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    secret_ref TEXT NOT NULL,
    secret_value TEXT NOT NULL,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    PRIMARY KEY (user_id, secret_ref)
);
