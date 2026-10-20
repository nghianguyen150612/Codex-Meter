CREATE TABLE token_usage_evidence (
    event_id TEXT NOT NULL PRIMARY KEY CHECK (length(event_id) BETWEEN 1 AND 128),
    source_id TEXT NOT NULL CHECK (length(source_id) BETWEEN 1 AND 128),
    session_id TEXT CHECK (session_id IS NULL OR length(session_id) BETWEEN 1 AND 128),
    task_id TEXT CHECK (task_id IS NULL OR length(task_id) BETWEEN 1 AND 128),
    observed_at TEXT NOT NULL CHECK (length(observed_at) BETWEEN 20 AND 128),
    record_semantic TEXT NOT NULL CHECK (record_semantic IN (
        'per_response',
        'turn_cumulative_snapshot',
        'thread_cumulative_snapshot',
        'token_count_total_snapshot',
        'token_count_latest_snapshot'
    )),
    model TEXT CHECK (model IS NULL OR length(model) BETWEEN 1 AND 256),
    reasoning_level TEXT CHECK (reasoning_level IS NULL OR length(reasoning_level) BETWEEN 1 AND 256),
    service_tier TEXT CHECK (service_tier IS NULL OR length(service_tier) BETWEEN 1 AND 256),
    codex_version TEXT CHECK (codex_version IS NULL OR length(codex_version) BETWEEN 1 AND 256),
    input_tokens INTEGER CHECK (input_tokens IS NULL OR input_tokens BETWEEN 0 AND 9007199254740991),
    cached_input_tokens INTEGER CHECK (cached_input_tokens IS NULL OR cached_input_tokens BETWEEN 0 AND 9007199254740991),
    cache_write_input_tokens INTEGER CHECK (cache_write_input_tokens IS NULL OR cache_write_input_tokens BETWEEN 0 AND 9007199254740991),
    output_tokens INTEGER CHECK (output_tokens IS NULL OR output_tokens BETWEEN 0 AND 9007199254740991),
    reasoning_output_tokens INTEGER CHECK (reasoning_output_tokens IS NULL OR reasoning_output_tokens BETWEEN 0 AND 9007199254740991),
    raw_total INTEGER CHECK (raw_total IS NULL OR raw_total BETWEEN 0 AND 9007199254740991),
    evidence_status TEXT NOT NULL CHECK (evidence_status IN ('observed', 'snapshot')),
    schema_version INTEGER NOT NULL CHECK (schema_version = 1),
    evidence_sha256 TEXT NOT NULL CHECK (length(evidence_sha256) = 64 AND evidence_sha256 NOT GLOB '*[^0-9a-f]*')
) STRICT;

CREATE INDEX token_usage_session_time_idx ON token_usage_evidence (session_id, observed_at, event_id);
CREATE INDEX token_usage_task_time_idx ON token_usage_evidence (task_id, observed_at, event_id);
CREATE INDEX token_usage_source_time_idx ON token_usage_evidence (source_id, observed_at, event_id);

CREATE TABLE quota_snapshot_evidence (
    sample_id TEXT NOT NULL PRIMARY KEY CHECK (length(sample_id) BETWEEN 1 AND 128),
    source_id TEXT NOT NULL CHECK (length(source_id) BETWEEN 1 AND 128),
    session_id TEXT CHECK (session_id IS NULL OR length(session_id) BETWEEN 1 AND 128),
    observed_at TEXT NOT NULL CHECK (length(observed_at) BETWEEN 20 AND 128),
    meter_type TEXT NOT NULL CHECK (meter_type IN ('five_hour', 'weekly')),
    used_percent REAL NOT NULL CHECK (used_percent >= 0 AND used_percent <= 100),
    reset_at TEXT,
    plan TEXT CHECK (plan IS NULL OR length(plan) BETWEEN 1 AND 64),
    codex_version TEXT CHECK (codex_version IS NULL OR length(codex_version) BETWEEN 1 AND 256),
    evidence_status TEXT NOT NULL CHECK (evidence_status = 'observed'),
    schema_version INTEGER NOT NULL CHECK (schema_version = 1),
    evidence_sha256 TEXT NOT NULL CHECK (length(evidence_sha256) = 64 AND evidence_sha256 NOT GLOB '*[^0-9a-f]*')
) STRICT;

CREATE INDEX quota_snapshot_meter_time_idx ON quota_snapshot_evidence (meter_type, observed_at DESC, sample_id DESC);

CREATE TABLE ingestion_diagnostics (
    diagnostic_id TEXT NOT NULL PRIMARY KEY CHECK (length(diagnostic_id) BETWEEN 1 AND 128),
    source_id TEXT NOT NULL CHECK (length(source_id) BETWEEN 1 AND 128),
    observed_at TEXT CHECK (observed_at IS NULL OR length(observed_at) BETWEEN 20 AND 128),
    record_semantic TEXT NOT NULL CHECK (length(record_semantic) BETWEEN 1 AND 64),
    diagnostic_code TEXT NOT NULL CHECK (diagnostic_code IN (
        'malformed_record',
        'empty_record',
        'invalid_utf8',
        'unsupported_record',
        'unsupported_event',
        'invalid_timestamp',
        'session_identity_conflict',
        'quota_unavailable',
        'quota_non_main_limit',
        'quota_unsupported_window',
        'quota_ambiguous_window',
        'source_changed',
        'source_truncated',
        'source_generation_ambiguous',
        'checkpoint_ambiguous',
        'checkpoint_corrupt',
        'source_read_limited',
        'source_read_failed',
        'source_transaction_conflict',
        'quota_evidence_incomplete',
        'unsupported_source_format',
        'discovery_scan_limited',
        'symlink_entry_skipped',
        'discovery_unreadable',
        'unsupported_codex_version',
        'incomplete_source_header',
        'ambiguous_source_identity',
        'compressed_source_unsupported',
        'data_coverage_incomplete'
    )),
    schema_version INTEGER NOT NULL CHECK (schema_version = 1),
    evidence_sha256 TEXT NOT NULL CHECK (length(evidence_sha256) = 64 AND evidence_sha256 NOT GLOB '*[^0-9a-f]*')
) STRICT;

CREATE INDEX ingestion_diagnostics_source_time_idx ON ingestion_diagnostics (source_id, observed_at, diagnostic_code);

CREATE TABLE source_boundary_fingerprints (
    rollout_id TEXT NOT NULL CHECK (length(rollout_id) BETWEEN 1 AND 128),
    source_generation TEXT NOT NULL CHECK (length(source_generation) BETWEEN 1 AND 128),
    committed_offset TEXT NOT NULL CHECK (length(committed_offset) BETWEEN 1 AND 20 AND committed_offset NOT GLOB '*[^0-9]*'),
    last_line_sha256 TEXT CHECK (last_line_sha256 IS NULL OR (length(last_line_sha256) = 64 AND last_line_sha256 NOT GLOB '*[^0-9a-f]*')),
    file_identity_sha256 TEXT CHECK (file_identity_sha256 IS NULL OR (length(file_identity_sha256) = 64 AND file_identity_sha256 NOT GLOB '*[^0-9a-f]*')),
    PRIMARY KEY (rollout_id, source_generation)
) STRICT;
