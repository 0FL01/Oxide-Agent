-- SQLx/Postgres storage foundation marker.
-- Business storage tables are added in later R2-to-Postgres porting phases.

CREATE TABLE IF NOT EXISTS oxide_storage_health (
    id TEXT PRIMARY KEY,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

INSERT INTO oxide_storage_health (id)
VALUES ('foundation')
ON CONFLICT (id) DO NOTHING;
