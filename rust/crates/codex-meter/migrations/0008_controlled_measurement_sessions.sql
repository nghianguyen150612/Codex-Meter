CREATE TABLE controlled_quota_measurement_sessions (
    session_id TEXT NOT NULL PRIMARY KEY CHECK (
        length(session_id) = 36 AND substr(session_id, 1, 4) = 'qms_'
        AND substr(session_id, 5) NOT GLOB '*[^0-9a-f]*'
    ),
    format_version INTEGER NOT NULL CHECK (format_version = 1),
    before_snapshot_id TEXT NOT NULL REFERENCES live_account_quota_snapshots(snapshot_id) ON DELETE RESTRICT,
    before_snapshot_sha256 TEXT NOT NULL CHECK (length(before_snapshot_sha256) = 64 AND before_snapshot_sha256 NOT GLOB '*[^0-9a-f]*'),
    created_at_utc TEXT NOT NULL CHECK (length(created_at_utc) BETWEEN 20 AND 40),
    created_at_unix_ns INTEGER NOT NULL,
    deadline_utc TEXT NOT NULL CHECK (length(deadline_utc) BETWEEN 20 AND 40),
    deadline_unix_ns INTEGER NOT NULL,
    max_duration_minutes INTEGER NOT NULL CHECK (max_duration_minutes BETWEEN 1 AND 240),
    state TEXT NOT NULL CHECK (state IN ('open', 'completed', 'aborted', 'expired')),
    after_snapshot_id TEXT REFERENCES live_account_quota_snapshots(snapshot_id) ON DELETE RESTRICT,
    after_snapshot_sha256 TEXT CHECK (after_snapshot_sha256 IS NULL OR (length(after_snapshot_sha256) = 64 AND after_snapshot_sha256 NOT GLOB '*[^0-9a-f]*')),
    completed_at_utc TEXT CHECK (completed_at_utc IS NULL OR length(completed_at_utc) BETWEEN 20 AND 40),
    completed_at_unix_ns INTEGER,
    activity_declaration TEXT CHECK (activity_declaration IS NULL OR activity_declaration IN ('unknown', 'known_local_overlap', 'self_reported_no_known_overlap')),
    revision INTEGER NOT NULL CHECK (revision >= 1),
    receipt_sha256 TEXT CHECK (receipt_sha256 IS NULL OR (length(receipt_sha256) = 64 AND receipt_sha256 NOT GLOB '*[^0-9a-f]*')),
    source TEXT NOT NULL CHECK (source = 'codex_app_server_account_rate_limits'),
    evidence_scope TEXT NOT NULL CHECK (evidence_scope = 'account_level'),
    task_attribution TEXT NOT NULL CHECK (task_attribution = 'not_established'),
    CHECK (
      (state = 'open' AND after_snapshot_id IS NULL AND after_snapshot_sha256 IS NULL AND completed_at_utc IS NULL AND completed_at_unix_ns IS NULL AND activity_declaration IS NULL AND receipt_sha256 IS NULL)
      OR (state = 'completed' AND after_snapshot_id IS NOT NULL AND after_snapshot_sha256 IS NOT NULL AND completed_at_utc IS NOT NULL AND completed_at_unix_ns IS NOT NULL AND activity_declaration IS NOT NULL AND receipt_sha256 IS NOT NULL)
      OR (state IN ('aborted', 'expired') AND after_snapshot_id IS NULL AND after_snapshot_sha256 IS NULL AND completed_at_utc IS NOT NULL AND completed_at_unix_ns IS NOT NULL AND activity_declaration IS NULL AND receipt_sha256 IS NULL)
    )
) STRICT;

CREATE UNIQUE INDEX controlled_quota_one_open_session
    ON controlled_quota_measurement_sessions ((1)) WHERE state = 'open';

CREATE TRIGGER controlled_quota_open_identity_immutable
BEFORE UPDATE ON controlled_quota_measurement_sessions
WHEN OLD.state = 'open' AND (
    NEW.session_id IS NOT OLD.session_id
    OR NEW.format_version IS NOT OLD.format_version
    OR NEW.before_snapshot_id IS NOT OLD.before_snapshot_id
    OR NEW.before_snapshot_sha256 IS NOT OLD.before_snapshot_sha256
    OR NEW.created_at_utc IS NOT OLD.created_at_utc
    OR NEW.created_at_unix_ns IS NOT OLD.created_at_unix_ns
    OR NEW.deadline_utc IS NOT OLD.deadline_utc
    OR NEW.deadline_unix_ns IS NOT OLD.deadline_unix_ns
    OR NEW.max_duration_minutes IS NOT OLD.max_duration_minutes
    OR NEW.source IS NOT OLD.source
    OR NEW.evidence_scope IS NOT OLD.evidence_scope
    OR NEW.task_attribution IS NOT OLD.task_attribution
)
BEGIN
    SELECT RAISE(ABORT, 'controlled quota measurement session identity is immutable');
END;

CREATE TRIGGER controlled_quota_session_receipt_immutable
BEFORE UPDATE ON controlled_quota_measurement_sessions
WHEN OLD.state IN ('completed', 'aborted', 'expired')
BEGIN
    SELECT RAISE(ABORT, 'terminal controlled quota measurement session is immutable');
END;

CREATE TRIGGER controlled_quota_session_delete_immutable
BEFORE DELETE ON controlled_quota_measurement_sessions
BEGIN
    SELECT RAISE(ABORT, 'controlled quota measurement session is immutable');
END;
