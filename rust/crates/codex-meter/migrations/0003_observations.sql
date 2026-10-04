CREATE TABLE observations (
    observation_id TEXT PRIMARY KEY NOT NULL CHECK (length(observation_id) > 0),
    schema_version TEXT NOT NULL CHECK (schema_version = '1.0.0'),
    task_id TEXT,
    session_id TEXT,
    source_instance_id TEXT,
    lifecycle_state TEXT NOT NULL CHECK (lifecycle_state IN ('detected', 'active', 'task_ended', 'awaiting_meter', 'reconciling', 'finalized', 'incomplete', 'invalid')),
    started_at TEXT,
    ended_at TEXT,
    finalized_at TEXT,
    duration_ms INTEGER CHECK (duration_ms IS NULL OR (duration_ms >= 0 AND duration_ms <= 9007199254740991)),
    summary_quality TEXT NOT NULL CHECK (summary_quality IN ('A', 'B', 'C', 'D', 'X')),
    plan TEXT,
    model TEXT,
    reasoning_level TEXT,
    speed_mode TEXT,
    codex_version TEXT,
    token_validity TEXT NOT NULL CHECK (token_validity IN ('valid', 'incomplete', 'invalid', 'unavailable')),
    token_quality TEXT NOT NULL CHECK (token_quality IN ('A', 'B', 'C', 'D', 'X')),
    raw_total INTEGER CHECK (raw_total IS NULL OR (raw_total >= 0 AND raw_total <= 9007199254740991)),
    five_hour_validity TEXT NOT NULL CHECK (five_hour_validity IN ('valid', 'incomplete', 'invalid', 'unavailable')),
    five_hour_quality TEXT NOT NULL CHECK (five_hour_quality IN ('A', 'B', 'C', 'D', 'X')),
    five_hour_delta REAL CHECK (five_hour_delta IS NULL OR (five_hour_delta >= 0 AND five_hour_delta <= 100)),
    five_hour_reset_status TEXT NOT NULL CHECK (five_hour_reset_status IN ('not_detected', 'detected', 'unavailable')),
    weekly_validity TEXT NOT NULL CHECK (weekly_validity IN ('valid', 'incomplete', 'invalid', 'unavailable')),
    weekly_quality TEXT NOT NULL CHECK (weekly_quality IN ('A', 'B', 'C', 'D', 'X')),
    weekly_delta REAL CHECK (weekly_delta IS NULL OR (weekly_delta >= 0 AND weekly_delta <= 100)),
    weekly_reset_status TEXT NOT NULL CHECK (weekly_reset_status IN ('not_detected', 'detected', 'unavailable')),
    observation_json TEXT NOT NULL CHECK (length(observation_json) > 0),
    observation_sha256 TEXT NOT NULL CHECK (length(observation_sha256) = 64 AND observation_sha256 NOT GLOB '*[^0-9a-f]*'),
    storage_revision INTEGER NOT NULL CHECK (storage_revision > 0),
    CHECK (json_valid(observation_json))
) STRICT;

CREATE INDEX observations_finalized_at_idx ON observations (finalized_at DESC, observation_id DESC);
CREATE INDEX observations_ended_at_idx ON observations (ended_at DESC, observation_id DESC);
CREATE INDEX observations_lifecycle_idx ON observations (lifecycle_state);
CREATE INDEX observations_plan_idx ON observations (plan);
CREATE INDEX observations_model_idx ON observations (model);
CREATE INDEX observations_reasoning_level_idx ON observations (reasoning_level);
CREATE INDEX observations_speed_mode_idx ON observations (speed_mode);
CREATE INDEX observations_summary_quality_idx ON observations (summary_quality);
CREATE INDEX observations_token_validity_idx ON observations (token_validity);
CREATE INDEX observations_five_hour_validity_idx ON observations (five_hour_validity);
CREATE INDEX observations_weekly_validity_idx ON observations (weekly_validity);
CREATE INDEX observations_configuration_history_idx
    ON observations (plan, model, reasoning_level, speed_mode, finalized_at DESC, observation_id DESC);
