-- Permanent Life Mode source-of-truth storage.
-- Scoped to web permanent chat: principals, identity links, turns, runs, inputs, events.
-- Engram/curator/memory-generation tables are intentionally excluded — they will be
-- added in a future migration when that subsystem is designed.

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
    transport_id TEXT NOT NULL CHECK (btrim(transport_id) <> ''),
    provider_subject TEXT NOT NULL,
    principal_user_id BIGINT NOT NULL REFERENCES life_principals(principal_user_id) ON DELETE CASCADE,
    verified_at BIGINT,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    PRIMARY KEY (transport_id, provider_subject)
);

CREATE INDEX life_identity_links_principal_idx
    ON life_identity_links (principal_user_id, transport_id);

CREATE TABLE life_transport_bindings (
    binding_id UUID PRIMARY KEY,
    principal_user_id BIGINT NOT NULL REFERENCES life_principals(principal_user_id) ON DELETE CASCADE,
    transport_id TEXT NOT NULL CHECK (btrim(transport_id) <> ''),
    inbound_address JSONB NOT NULL CHECK (jsonb_typeof(inbound_address) = 'object'),
    delivery_address JSONB NOT NULL CHECK (jsonb_typeof(delivery_address) = 'object'),
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    UNIQUE (transport_id, inbound_address)
);

CREATE INDEX life_transport_bindings_principal_idx
    ON life_transport_bindings (principal_user_id, transport_id);

CREATE INDEX life_transport_bindings_enabled_idx
    ON life_transport_bindings (transport_id, enabled);

CREATE TABLE life_turns (
    turn_id UUID PRIMARY KEY,
    principal_user_id BIGINT NOT NULL REFERENCES life_principals(principal_user_id) ON DELETE CASCADE,
    run_id UUID,
    role TEXT NOT NULL CHECK (role IN ('user', 'assistant', 'system', 'tool')),
    source_transport TEXT NOT NULL CHECK (btrim(source_transport) <> ''),
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
    status TEXT NOT NULL CHECK (status IN ('queued', 'running', 'completed', 'failed', 'cancelled')),
    started_at BIGINT,
    finished_at BIGINT,
    last_checkpoint_at BIGINT,
    error_text TEXT,
    lease_owner TEXT,
    lease_expires_at BIGINT,
    last_heartbeat_at BIGINT,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    CHECK (
        status <> 'running'
        OR (
            lease_owner IS NOT NULL
            AND btrim(lease_owner) <> ''
            AND lease_expires_at IS NOT NULL
            AND last_heartbeat_at IS NOT NULL
        )
    )
);

CREATE INDEX life_runs_principal_status_idx
    ON life_runs (principal_user_id, status, updated_at DESC);

CREATE UNIQUE INDEX life_runs_one_running_per_principal_idx
    ON life_runs (principal_user_id)
    WHERE status = 'running';

CREATE INDEX life_runs_running_lease_idx
    ON life_runs (principal_user_id, lease_expires_at)
    WHERE status = 'running';

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
