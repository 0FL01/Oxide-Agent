CREATE INDEX IF NOT EXISTS memory_episodes_lexical_search_idx
    ON memory_episodes
    USING GIN (
        to_tsvector(
            'simple',
            coalesce(goal, '') || ' ' ||
            coalesce(summary, '') || ' ' ||
            coalesce(array_to_string(tools_used, ' '), '') || ' ' ||
            coalesce(array_to_string(failures, ' '), '')
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
            coalesce(array_to_string(tags, ' '), '')
        )
    );
