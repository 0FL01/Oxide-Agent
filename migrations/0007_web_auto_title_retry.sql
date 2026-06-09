ALTER TABLE web_sessions
    ADD COLUMN auto_title_source_message TEXT,
    ADD COLUMN auto_title_replaceable_title TEXT,
    ADD COLUMN auto_title_attempts INTEGER NOT NULL DEFAULT 0 CHECK (auto_title_attempts >= 0),
    ADD COLUMN auto_title_next_attempt_at TIMESTAMPTZ,
    ADD COLUMN auto_title_last_error TEXT;

CREATE INDEX web_sessions_auto_title_due_idx
    ON web_sessions (auto_title_next_attempt_at, updated_at)
    WHERE auto_title_source_message IS NOT NULL AND manually_renamed = FALSE;
