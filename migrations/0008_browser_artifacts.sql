-- 0008_browser_artifacts.sql
-- Browser screenshot artifacts stored as BYTEA in Postgres.
-- Replaces the filesystem-only pipeline (raw PNG on disk).
-- Screenshots are JPEG (CDP Page.captureScreenshot format=jpeg,quality=80).

CREATE TABLE browser_artifacts (
    artifact_uri TEXT PRIMARY KEY,
    user_id BIGINT NOT NULL,
    session_id TEXT NOT NULL,
    task_id TEXT NOT NULL,
    mime_type TEXT NOT NULL DEFAULT 'image/jpeg',
    data BYTEA NOT NULL,
    bytes BIGINT NOT NULL CHECK (bytes >= 0),
    sha256 TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    FOREIGN KEY (user_id, session_id, task_id)
        REFERENCES web_tasks(user_id, session_id, task_id)
        ON DELETE CASCADE
);

CREATE INDEX browser_artifacts_session_idx
    ON browser_artifacts (session_id);

CREATE INDEX browser_artifacts_created_idx
    ON browser_artifacts (created_at);
