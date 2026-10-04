CREATE TABLE runtime_checkpoints (
    rollout_id TEXT NOT NULL CHECK (length(rollout_id) BETWEEN 1 AND 128),
    source_generation TEXT NOT NULL CHECK (length(source_generation) BETWEEN 1 AND 128),
    committed_offset TEXT NOT NULL CHECK (
        length(committed_offset) BETWEEN 1 AND 20
        AND committed_offset NOT GLOB '*[^0-9]*'
    ),
    last_ordinal TEXT CHECK (
        last_ordinal IS NULL OR (
            length(last_ordinal) BETWEEN 1 AND 20
            AND last_ordinal NOT GLOB '*[^0-9]*'
        )
    ),
    checkpoint_revision INTEGER NOT NULL CHECK (checkpoint_revision > 0),
    state_format_version INTEGER NOT NULL CHECK (state_format_version > 0),
    state_json TEXT NOT NULL CHECK (length(state_json) > 0),
    state_sha256 TEXT NOT NULL CHECK (
        length(state_sha256) = 64
        AND state_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    PRIMARY KEY (rollout_id, source_generation)
) STRICT;
