-- array_to_string is STABLE, not IMMUTABLE, and cannot be used in index expressions.
-- Create a minimal IMMUTABLE wrapper so GIN indexes can use it.
CREATE OR REPLACE FUNCTION imm_array_to_string(text[], text)
RETURNS text
LANGUAGE sql IMMUTABLE STRICT AS $$
    SELECT array_to_string($1, $2)
$$;

CREATE INDEX IF NOT EXISTS memory_episodes_lexical_search_idx
    ON memory_episodes
    USING GIN (
        to_tsvector(
            'simple',
            coalesce(goal, '') || ' ' ||
            coalesce(summary, '') || ' ' ||
            coalesce(imm_array_to_string(tools_used, ' '), '') || ' ' ||
            coalesce(imm_array_to_string(failures, ' '), '')
        )
    );

CREATE INDEX IF NOT EXISTS memory_records_lexical_search_idx
    ON memory_records
    USING GIN (
        to_tsvector(
            'simple',
            coalesce(title, '') || ' ' ||
            coalesce(short_description, '') || ' ' ||
            coalesce(content, '') || ' ' ||
            coalesce(imm_array_to_string(tags, ' '), '')
        )
    );
