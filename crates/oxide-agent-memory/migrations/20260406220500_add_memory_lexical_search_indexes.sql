CREATE INDEX IF NOT EXISTS memory_episodes_lexical_search_idx
    ON memory_episodes
    USING GIN (
        to_tsvector(
            'simple',
            concat_ws(
                ' ',
                goal,
                summary,
                array_to_string(tools_used, ' '),
                array_to_string(failures, ' ')
            )
        )
    );

CREATE INDEX IF NOT EXISTS memory_records_lexical_search_idx
    ON memory_records
    USING GIN (
        to_tsvector(
            'simple',
            concat_ws(
                ' ',
                title,
                short_description,
                content,
                array_to_string(tags, ' ')
            )
        )
    );
