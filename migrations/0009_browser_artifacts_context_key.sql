-- 0009_browser_artifacts_context_key.sql
-- Refactor: replace web_tasks FK with transport-agnostic context_key.
--
-- Root cause: the browser provider (oxide-agent-core) does not have access
-- to web_tasks IDs (user_id, session_id, task_id). It operates in the
-- transport-agnostic core layer and only has AgentMemoryScope identifiers:
-- user_id and context_key. The FK to web_tasks was a contract bug — the
-- sending side could not reliably form the required
-- (user_id, session_id, task_id) tuple.
--
-- Fix: use context_key (transport-agnostic session identifier) for
-- deletion. When a web session is deleted, the transport calls
-- delete_browser_artifacts_by_context_key(user_id, context_key).
-- No FK — explicit cleanup is clearer and avoids the contract mismatch.

-- 1. Drop the web_tasks FK.
ALTER TABLE browser_artifacts
    DROP CONSTRAINT IF EXISTS browser_artifacts_user_id_session_id_task_id_fkey;

-- 2. Add context_key column (transport-agnostic session identifier).
ALTER TABLE browser_artifacts
    ADD COLUMN IF NOT EXISTS context_key TEXT NOT NULL DEFAULT '';

-- 3. Replace session_id index with context_key index.
DROP INDEX IF EXISTS browser_artifacts_session_idx;
CREATE INDEX IF NOT EXISTS browser_artifacts_context_idx
    ON browser_artifacts (user_id, context_key);

-- 4. Keep created_at index for retention queries.
-- (browser_artifacts_created_idx already exists from 0008.)
