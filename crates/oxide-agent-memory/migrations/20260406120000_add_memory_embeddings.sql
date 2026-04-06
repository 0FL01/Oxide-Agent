CREATE EXTENSION IF NOT EXISTS vector;

CREATE TABLE IF NOT EXISTS memory_embeddings (
    owner_id TEXT NOT NULL,
    owner_type TEXT NOT NULL CHECK (owner_type IN ('episode', 'memory')),
    model_id TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    embedding VECTOR NULL,
    dimensions INTEGER NULL CHECK (dimensions IS NULL OR dimensions > 0),
    status TEXT NOT NULL CHECK (status IN ('pending', 'ready', 'failed')),
    last_error TEXT NULL,
    retry_count INTEGER NOT NULL DEFAULT 0 CHECK (retry_count >= 0),
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    indexed_at TIMESTAMPTZ NULL,
    PRIMARY KEY (owner_type, owner_id)
);

CREATE INDEX IF NOT EXISTS memory_embeddings_status_updated_idx
    ON memory_embeddings (status, updated_at ASC, owner_type, owner_id);

CREATE INDEX IF NOT EXISTS memory_embeddings_model_status_updated_idx
    ON memory_embeddings (model_id, status, updated_at ASC, owner_type, owner_id);

CREATE INDEX IF NOT EXISTS memory_embeddings_vector_idx
    ON memory_embeddings
    USING ivfflat (embedding vector_cosine_ops)
    WITH (lists = 64)
    WHERE status = 'ready' AND embedding IS NOT NULL;
