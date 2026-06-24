-- Permanent Life Mode source-of-truth storage.

CREATE TABLE life_principals (
    principal_user_id BIGINT PRIMARY KEY REFERENCES users(user_id) ON DELETE CASCADE,
    profile_state JSONB NOT NULL DEFAULT '{}'::jsonb,
    operating_profile JSONB NOT NULL DEFAULT '{}'::jsonb,
    settings JSONB NOT NULL DEFAULT '{}'::jsonb,
    schema_version INTEGER NOT NULL DEFAULT 1,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL
);

CREATE TABLE life_identity_links (
    provider TEXT NOT NULL CHECK (provider IN ('web', 'telegram')),
    provider_subject TEXT NOT NULL,
    principal_user_id BIGINT NOT NULL REFERENCES life_principals(principal_user_id) ON DELETE CASCADE,
    verified_at BIGINT,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    PRIMARY KEY (provider, provider_subject)
);

CREATE INDEX life_identity_links_principal_idx
    ON life_identity_links (principal_user_id, provider);

CREATE TABLE life_link_tokens (
    token_hash TEXT PRIMARY KEY,
    principal_user_id BIGINT NOT NULL REFERENCES life_principals(principal_user_id) ON DELETE CASCADE,
    target_provider TEXT NOT NULL CHECK (target_provider IN ('web', 'telegram')),
    expires_at BIGINT NOT NULL,
    consumed_at BIGINT,
    created_at BIGINT NOT NULL,
    CHECK (consumed_at IS NULL OR consumed_at >= created_at)
);

CREATE INDEX life_link_tokens_principal_idx
    ON life_link_tokens (principal_user_id, expires_at);

CREATE TABLE life_memory_generations (
    memory_generation_id UUID PRIMARY KEY,
    principal_user_id BIGINT NOT NULL REFERENCES life_principals(principal_user_id) ON DELETE CASCADE,
    generation_number BIGINT NOT NULL CHECK (generation_number >= 1),
    status TEXT NOT NULL CHECK (status IN ('building', 'active', 'archived', 'failed', 'deleted')),
    source_generation_id UUID REFERENCES life_memory_generations(memory_generation_id) ON DELETE SET NULL,
    build_reason TEXT NOT NULL,
    build_policy JSONB NOT NULL DEFAULT '{}'::jsonb,
    source_scope JSONB NOT NULL DEFAULT '{}'::jsonb,
    comparison_report JSONB NOT NULL DEFAULT '{}'::jsonb,
    activated_at BIGINT,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    UNIQUE (principal_user_id, generation_number),
    UNIQUE (principal_user_id, memory_generation_id)
);

CREATE UNIQUE INDEX life_memory_generations_one_active_idx
    ON life_memory_generations (principal_user_id)
    WHERE status = 'active';

CREATE INDEX life_memory_generations_principal_status_idx
    ON life_memory_generations (principal_user_id, status, generation_number DESC);

CREATE TABLE life_active_memory_generations (
    principal_user_id BIGINT PRIMARY KEY REFERENCES life_principals(principal_user_id) ON DELETE CASCADE,
    memory_generation_id UUID NOT NULL,
    activated_at BIGINT NOT NULL,
    activation_reason TEXT NOT NULL,
    FOREIGN KEY (principal_user_id, memory_generation_id)
        REFERENCES life_memory_generations (principal_user_id, memory_generation_id)
        ON DELETE RESTRICT
);

CREATE TABLE life_turns (
    turn_id UUID PRIMARY KEY,
    principal_user_id BIGINT NOT NULL REFERENCES life_principals(principal_user_id) ON DELETE CASCADE,
    run_id UUID,
    role TEXT NOT NULL CHECK (role IN ('user', 'assistant', 'system', 'tool')),
    source_transport TEXT NOT NULL CHECK (source_transport IN ('web', 'telegram', 'internal')),
    source_ref TEXT,
    content TEXT NOT NULL,
    attachments JSONB NOT NULL DEFAULT '[]'::jsonb,
    transport_metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
    redaction_state TEXT NOT NULL DEFAULT 'clean' CHECK (redaction_state IN ('clean', 'redacted', 'secret_blocked')),
    created_at BIGINT NOT NULL
);

CREATE INDEX life_turns_principal_created_idx
    ON life_turns (principal_user_id, created_at DESC);

CREATE TABLE life_runs (
    run_id UUID PRIMARY KEY,
    principal_user_id BIGINT NOT NULL REFERENCES life_principals(principal_user_id) ON DELETE CASCADE,
    memory_generation_id UUID NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('queued', 'running', 'completed', 'failed', 'cancelled')),
    started_at BIGINT,
    finished_at BIGINT,
    last_checkpoint_at BIGINT,
    error_text TEXT,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    FOREIGN KEY (principal_user_id, memory_generation_id)
        REFERENCES life_memory_generations (principal_user_id, memory_generation_id)
        ON DELETE RESTRICT
);

CREATE INDEX life_runs_principal_status_idx
    ON life_runs (principal_user_id, status, updated_at DESC);

CREATE TABLE life_inputs (
    input_id UUID PRIMARY KEY,
    principal_user_id BIGINT NOT NULL REFERENCES life_principals(principal_user_id) ON DELETE CASCADE,
    turn_id UUID NOT NULL REFERENCES life_turns(turn_id) ON DELETE CASCADE,
    status TEXT NOT NULL CHECK (status IN ('queued', 'claimed', 'consumed', 'dead')),
    claimed_by TEXT,
    claimed_at BIGINT,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL
);

CREATE INDEX life_inputs_claim_idx
    ON life_inputs (principal_user_id, status, created_at ASC)
    WHERE status IN ('queued', 'claimed');

ALTER TABLE life_turns
    ADD CONSTRAINT life_turns_run_fk
    FOREIGN KEY (run_id) REFERENCES life_runs(run_id) ON DELETE SET NULL;

CREATE TABLE life_events (
    event_id UUID PRIMARY KEY,
    run_id UUID NOT NULL REFERENCES life_runs(run_id) ON DELETE CASCADE,
    seq BIGINT NOT NULL CHECK (seq >= 0),
    kind TEXT NOT NULL,
    payload JSONB NOT NULL,
    created_at BIGINT NOT NULL,
    UNIQUE (run_id, seq)
);

CREATE TABLE life_context_overrides (
    override_id UUID PRIMARY KEY,
    principal_user_id BIGINT NOT NULL REFERENCES life_principals(principal_user_id) ON DELETE CASCADE,
    key TEXT NOT NULL,
    value JSONB NOT NULL,
    reason TEXT,
    expires_at BIGINT,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL
);

CREATE INDEX life_context_overrides_active_idx
    ON life_context_overrides (principal_user_id, key, expires_at);

CREATE TABLE life_memory_items (
    memory_id UUID PRIMARY KEY,
    principal_user_id BIGINT NOT NULL REFERENCES life_principals(principal_user_id) ON DELETE CASCADE,
    memory_generation_id UUID NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN (
        'biography',
        'preference',
        'project_principle',
        'procedure',
        'decision',
        'episode',
        'operating_rule',
        'friction_pattern',
        'support_protocol'
    )),
    authority TEXT NOT NULL CHECK (authority IN ('user_asserted', 'user_confirmed', 'curator_suggested', 'system_derived')),
    status TEXT NOT NULL CHECK (status IN ('active', 'superseded', 'deleted', 'candidate')),
    text TEXT NOT NULL,
    structured JSONB NOT NULL DEFAULT '{}'::jsonb,
    tags TEXT[] NOT NULL DEFAULT '{}',
    evidence_turn_ids UUID[] NOT NULL DEFAULT '{}',
    sensitivity TEXT NOT NULL CHECK (sensitivity IN ('clean', 'personal', 'redacted', 'secret_blocked')),
    valid_from BIGINT,
    valid_to BIGINT,
    supersedes_memory_id UUID REFERENCES life_memory_items(memory_id) ON DELETE SET NULL,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    FOREIGN KEY (principal_user_id, memory_generation_id)
        REFERENCES life_memory_generations (principal_user_id, memory_generation_id)
        ON DELETE CASCADE
);

CREATE INDEX life_memory_items_active_scope_idx
    ON life_memory_items (principal_user_id, memory_generation_id, status, updated_at DESC);

CREATE TABLE life_task_states (
    task_state_id UUID PRIMARY KEY,
    principal_user_id BIGINT NOT NULL REFERENCES life_principals(principal_user_id) ON DELETE CASCADE,
    memory_generation_id UUID NOT NULL,
    project_key TEXT NOT NULL,
    current_goal TEXT NOT NULL,
    why TEXT,
    current_state JSONB NOT NULL DEFAULT '[]'::jsonb,
    next_action TEXT,
    open_loops JSONB NOT NULL DEFAULT '[]'::jsonb,
    blockers JSONB NOT NULL DEFAULT '[]'::jsonb,
    status TEXT NOT NULL CHECK (status IN ('active', 'paused', 'completed', 'abandoned')),
    last_turn_id UUID REFERENCES life_turns(turn_id) ON DELETE SET NULL,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    UNIQUE (principal_user_id, memory_generation_id, project_key),
    FOREIGN KEY (principal_user_id, memory_generation_id)
        REFERENCES life_memory_generations (principal_user_id, memory_generation_id)
        ON DELETE CASCADE
);

CREATE INDEX life_task_states_active_scope_idx
    ON life_task_states (principal_user_id, memory_generation_id, status, updated_at DESC);

CREATE TABLE life_friction_patterns (
    pattern_id UUID PRIMARY KEY,
    principal_user_id BIGINT NOT NULL REFERENCES life_principals(principal_user_id) ON DELETE CASCADE,
    memory_generation_id UUID NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('overload_trigger', 'task_initiation_barrier', 'context_loss', 'communication_mismatch', 'sensory_or_energy_constraint')),
    trigger_descriptor TEXT NOT NULL,
    preferred_response JSONB NOT NULL,
    evidence_turn_ids UUID[] NOT NULL DEFAULT '{}',
    authority TEXT NOT NULL CHECK (authority IN ('user_asserted', 'user_confirmed', 'curator_suggested')),
    status TEXT NOT NULL CHECK (status IN ('active', 'superseded', 'deleted', 'candidate')),
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    FOREIGN KEY (principal_user_id, memory_generation_id)
        REFERENCES life_memory_generations (principal_user_id, memory_generation_id)
        ON DELETE CASCADE
);

CREATE INDEX life_friction_patterns_active_scope_idx
    ON life_friction_patterns (principal_user_id, memory_generation_id, status, updated_at DESC);

CREATE TABLE life_support_protocols (
    protocol_id UUID PRIMARY KEY,
    principal_user_id BIGINT NOT NULL REFERENCES life_principals(principal_user_id) ON DELETE CASCADE,
    memory_generation_id UUID NOT NULL,
    name TEXT NOT NULL,
    trigger_descriptor TEXT NOT NULL,
    steps JSONB NOT NULL,
    priority INTEGER NOT NULL DEFAULT 0,
    evidence_turn_ids UUID[] NOT NULL DEFAULT '{}',
    authority TEXT NOT NULL CHECK (authority IN ('user_asserted', 'user_confirmed', 'curator_suggested')),
    status TEXT NOT NULL CHECK (status IN ('active', 'superseded', 'deleted', 'candidate')),
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    FOREIGN KEY (principal_user_id, memory_generation_id)
        REFERENCES life_memory_generations (principal_user_id, memory_generation_id)
        ON DELETE CASCADE
);

CREATE INDEX life_support_protocols_active_scope_idx
    ON life_support_protocols (principal_user_id, memory_generation_id, status, priority DESC, updated_at DESC);

CREATE TABLE life_engram_outbox (
    outbox_id UUID PRIMARY KEY,
    principal_user_id BIGINT NOT NULL REFERENCES life_principals(principal_user_id) ON DELETE CASCADE,
    memory_generation_id UUID NOT NULL,
    source_memory_id UUID REFERENCES life_memory_items(memory_id) ON DELETE SET NULL,
    idempotency_key TEXT NOT NULL UNIQUE,
    payload JSONB NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('pending', 'flushing', 'flushed', 'dead')),
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    next_attempt_at BIGINT NOT NULL,
    last_error TEXT,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    FOREIGN KEY (principal_user_id, memory_generation_id)
        REFERENCES life_memory_generations (principal_user_id, memory_generation_id)
        ON DELETE CASCADE
);

CREATE INDEX life_engram_outbox_due_idx
    ON life_engram_outbox (status, next_attempt_at ASC)
    WHERE status IN ('pending', 'flushing');

CREATE INDEX life_engram_outbox_scope_idx
    ON life_engram_outbox (principal_user_id, memory_generation_id, status);
