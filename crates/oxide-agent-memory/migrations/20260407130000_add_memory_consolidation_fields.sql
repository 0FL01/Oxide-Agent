ALTER TABLE memory_records
    ADD COLUMN IF NOT EXISTS content_hash TEXT NULL,
    ADD COLUMN IF NOT EXISTS deleted_at TIMESTAMPTZ NULL;

CREATE INDEX IF NOT EXISTS memory_records_context_deleted_updated_idx
    ON memory_records (context_key, deleted_at, updated_at DESC);

CREATE INDEX IF NOT EXISTS memory_records_context_type_hash_idx
    ON memory_records (context_key, memory_type, content_hash)
    WHERE content_hash IS NOT NULL;
