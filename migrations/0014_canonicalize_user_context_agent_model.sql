UPDATE user_contexts
SET agent_model_qualified_id = btrim(agent_model_qualified_id)
WHERE agent_model_qualified_id IS NOT NULL;

UPDATE user_contexts
SET agent_model_qualified_id = substr(
    agent_model_qualified_id,
    length('llm-provider/') + 1
)
WHERE agent_model_qualified_id LIKE 'llm-provider/%';

UPDATE user_contexts
SET agent_model_qualified_id = regexp_replace(
    agent_model_qualified_id,
    '^(opencode-go|opencode-zen)/\1/',
    '\1/'
)
WHERE agent_model_qualified_id ~ '^(opencode-go|opencode-zen)/(opencode-go|opencode-zen)/';

UPDATE user_contexts
SET agent_model_qualified_id = regexp_replace(
    agent_model_qualified_id,
    '^openai-base:([^/]+)/openai-base:\1/',
    'openai-base:\1/'
)
WHERE agent_model_qualified_id ~ '^openai-base:([^/]+)/openai-base:[^/]+/';

DO $$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM user_contexts
        WHERE agent_model_qualified_id IS NOT NULL
          AND (
              agent_model_qualified_id = ''
              OR agent_model_qualified_id LIKE 'llm-provider/%'
              OR position('/' IN agent_model_qualified_id) <= 1
              OR substr(
                  agent_model_qualified_id,
                  position('/' IN agent_model_qualified_id) + 1
              ) = ''
          )
    ) THEN
        RAISE EXCEPTION 'Cannot canonicalize malformed user_contexts.agent_model_qualified_id';
    END IF;
END
$$;
