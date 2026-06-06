-- Final SQLx/Postgres hardening indexes for bounded cleanup paths.

CREATE INDEX auth_sessions_revoked_cleanup_idx
    ON auth_sessions (revoked_at)
    WHERE revoked_at IS NOT NULL;
