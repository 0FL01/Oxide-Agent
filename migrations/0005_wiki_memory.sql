-- SQLx/Postgres durable wiki memory storage.

CREATE TABLE wiki_pages (
    storage_prefix TEXT NOT NULL DEFAULT '',
    scope_kind TEXT NOT NULL CHECK (scope_kind IN ('global', 'context')),
    context_id TEXT NOT NULL DEFAULT '',
    item_kind TEXT NOT NULL CHECK (item_kind IN ('global', 'core', 'page', 'inbox', 'raw')),
    path TEXT NOT NULL,
    content TEXT NOT NULL,
    content_bytes BIGINT NOT NULL CHECK (content_bytes >= 0),
    retention_expires_at BIGINT,
    version BIGINT NOT NULL DEFAULT 1 CHECK (version >= 1),
    schema_version INTEGER NOT NULL DEFAULT 1,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    PRIMARY KEY (storage_prefix, scope_kind, context_id, path),
    CHECK ((scope_kind = 'global' AND context_id = '') OR scope_kind = 'context'),
    CHECK (
        (item_kind = 'inbox' AND content_bytes <= 16384)
        OR (item_kind <> 'inbox' AND content_bytes <= 65536)
    )
);

CREATE INDEX wiki_pages_context_idx
    ON wiki_pages (context_id, item_kind, path)
    WHERE scope_kind = 'context';

CREATE INDEX wiki_pages_prefix_scope_idx
    ON wiki_pages (storage_prefix, scope_kind, context_id, item_kind);

CREATE INDEX wiki_pages_retention_idx
    ON wiki_pages (retention_expires_at)
    WHERE retention_expires_at IS NOT NULL;
