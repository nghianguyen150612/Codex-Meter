CREATE TABLE live_account_quota_snapshots (
    snapshot_id TEXT NOT NULL PRIMARY KEY CHECK (
        length(snapshot_id) = 36
        AND substr(snapshot_id, 1, 4) = 'lqs_'
        AND substr(snapshot_id, 5) NOT GLOB '*[^0-9a-f]*'
    ),
    acquired_at_utc TEXT NOT NULL CHECK (length(acquired_at_utc) BETWEEN 20 AND 40),
    acquired_at_unix_ns INTEGER NOT NULL,
    storage_schema_version INTEGER NOT NULL CHECK (storage_schema_version = 1),
    source TEXT NOT NULL CHECK (source = 'codex_app_server_account_rate_limits'),
    evidence_scope TEXT NOT NULL CHECK (evidence_scope = 'account_level'),
    task_attribution TEXT NOT NULL CHECK (task_attribution = 'not_established'),
    forecast_qualification TEXT NOT NULL CHECK (forecast_qualification = 'not_established'),
    overall_availability TEXT NOT NULL CHECK (overall_availability IN (
        'available', 'partial', 'unavailable', 'ambiguous'
    )),
    main_bucket_identity TEXT NOT NULL CHECK (main_bucket_identity IN (
        'main_codex', 'other_codex_related', 'other_bucket', 'unknown', 'conflicting'
    )),
    five_hour_availability TEXT NOT NULL CHECK (five_hour_availability IN (
        'available', 'unavailable', 'ambiguous'
    )),
    five_hour_used_percent REAL CHECK (
        five_hour_used_percent IS NULL OR
        (five_hour_used_percent >= 0 AND five_hour_used_percent <= 100)
    ),
    five_hour_reset_at_utc TEXT CHECK (
        five_hour_reset_at_utc IS NULL OR length(five_hour_reset_at_utc) BETWEEN 20 AND 40
    ),
    weekly_availability TEXT NOT NULL CHECK (weekly_availability IN (
        'available', 'unavailable', 'ambiguous'
    )),
    weekly_used_percent REAL CHECK (
        weekly_used_percent IS NULL OR
        (weekly_used_percent >= 0 AND weekly_used_percent <= 100)
    ),
    weekly_reset_at_utc TEXT CHECK (
        weekly_reset_at_utc IS NULL OR length(weekly_reset_at_utc) BETWEEN 20 AND 40
    ),
    warning_codes TEXT NOT NULL CHECK (
        length(warning_codes) <= 1024 AND json_valid(warning_codes)
        AND json_type(warning_codes) = 'array'
    ),
    payload_sha256 TEXT NOT NULL CHECK (
        length(payload_sha256) = 64 AND payload_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    CHECK (
        (five_hour_availability = 'available' AND five_hour_used_percent IS NOT NULL)
        OR (five_hour_availability <> 'available' AND five_hour_used_percent IS NULL)
    ),
    CHECK (
        (weekly_availability = 'available' AND weekly_used_percent IS NOT NULL)
        OR (weekly_availability <> 'available' AND weekly_used_percent IS NULL)
    ),
    CHECK (
        (five_hour_availability = 'available' OR five_hour_reset_at_utc IS NULL)
        AND (weekly_availability = 'available' OR weekly_reset_at_utc IS NULL)
    ),
    CHECK (
        (overall_availability = 'available'
            AND five_hour_availability = 'available' AND weekly_availability = 'available')
        OR (overall_availability = 'partial'
            AND ((five_hour_availability = 'available') <> (weekly_availability = 'available')))
        OR (overall_availability = 'unavailable'
            AND five_hour_availability = 'unavailable' AND weekly_availability = 'unavailable')
        OR (overall_availability = 'ambiguous'
            AND (five_hour_availability = 'ambiguous' OR weekly_availability = 'ambiguous'))
    ),
    CHECK (
        main_bucket_identity = 'main_codex'
        OR (five_hour_availability <> 'available' AND weekly_availability <> 'available')
    ),
    CHECK (main_bucket_identity <> 'conflicting' OR overall_availability = 'ambiguous')
) STRICT;

CREATE INDEX live_account_quota_snapshots_acquired_idx
    ON live_account_quota_snapshots (acquired_at_unix_ns DESC, snapshot_id DESC);
