//! Durable local sessions that organize two explicitly saved account snapshots.
//! These records do not represent task execution or task-specific quota use.

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use sha2::{Digest, Sha256};
use time::{format_description::well_known::Rfc3339, OffsetDateTime, UtcOffset};

use super::live_account_snapshots::{load_snapshot_by_id, validate_snapshot_id};
use super::sqlite::{SqliteReadStore, SqliteStore, StorageError};
use crate::telemetry::evaluate_live_quota_comparison;

pub const CONTROLLED_MEASUREMENT_FORMAT_VERSION: u32 = 1;
pub const EVALUATOR_CONTRACT_VERSION: &str = "1.0.0";
pub const DEFAULT_MEASUREMENT_MINUTES: u16 = 60;
pub const MAX_MEASUREMENT_MINUTES: u16 = 240;
const BASELINE_FRESHNESS_SECONDS: i64 = 300;
const SESSION_SOURCE: &str = "codex_app_server_account_rate_limits";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MeasurementSessionState {
    Open,
    Completed,
    Aborted,
    Expired,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityDeclaration {
    Unknown,
    KnownLocalOverlap,
    SelfReportedNoKnownOverlap,
}

impl ActivityDeclaration {
    fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::KnownLocalOverlap => "known_local_overlap",
            Self::SelfReportedNoKnownOverlap => "self_reported_no_known_overlap",
        }
    }

    fn parse(value: &str) -> Result<Self, StorageError> {
        match value {
            "unknown" => Ok(Self::Unknown),
            "known_local_overlap" => Ok(Self::KnownLocalOverlap),
            "self_reported_no_known_overlap" => Ok(Self::SelfReportedNoKnownOverlap),
            _ => Err(StorageError::ControlledMeasurementCorrupt),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ControlledMeasurementSession {
    pub session_id: String,
    pub format_version: u32,
    pub state: MeasurementSessionState,
    pub before_snapshot_id: String,
    pub before_snapshot_sha256: String,
    pub created_at_utc: String,
    pub deadline_utc: String,
    pub max_duration_minutes: u16,
    pub after_snapshot_id: Option<String>,
    pub after_snapshot_sha256: Option<String>,
    pub completed_at_utc: Option<String>,
    pub activity_declaration: Option<ActivityDeclaration>,
    pub revision: u64,
    pub receipt_sha256: Option<String>,
    pub source: &'static str,
    pub evidence_scope: &'static str,
    pub task_attribution: &'static str,
}

#[derive(Serialize)]
struct TerminalReceipt<'a> {
    receipt_version: u32,
    session_id: &'a str,
    lifecycle_state: MeasurementSessionState,
    before_snapshot_id: &'a str,
    before_snapshot_sha256: &'a str,
    after_snapshot_id: &'a str,
    after_snapshot_sha256: &'a str,
    policy_version: u32,
    max_duration_minutes: u16,
    activity_declaration: ActivityDeclaration,
    completed_at_utc: &'a str,
    evaluator_contract_version: &'static str,
    source: &'static str,
    evidence_scope: &'static str,
    task_attribution: &'static str,
}

impl SqliteStore {
    pub fn start_controlled_measurement(
        &mut self,
        before_snapshot_id: &str,
        max_duration_minutes: u16,
    ) -> Result<ControlledMeasurementSession, StorageError> {
        self.start_controlled_measurement_inner(before_snapshot_id, max_duration_minutes, None)
    }

    #[cfg(test)]
    pub(crate) fn start_controlled_measurement_at(
        &mut self,
        before_snapshot_id: &str,
        max_duration_minutes: u16,
        now: OffsetDateTime,
    ) -> Result<ControlledMeasurementSession, StorageError> {
        self.start_controlled_measurement_inner(before_snapshot_id, max_duration_minutes, Some(now))
    }

    fn start_controlled_measurement_inner(
        &mut self,
        before_snapshot_id: &str,
        max_duration_minutes: u16,
        requested_now: Option<OffsetDateTime>,
    ) -> Result<ControlledMeasurementSession, StorageError> {
        validate_snapshot_id(before_snapshot_id)
            .map_err(|_| StorageError::ControlledMeasurementInvalid)?;
        if !(1..=MAX_MEASUREMENT_MINUTES).contains(&max_duration_minutes) {
            return Err(StorageError::ControlledMeasurementInvalid);
        }
        let session_random: String = self
            .connection
            .query_row("SELECT lower(hex(randomblob(16)))", [], |row| row.get(0))
            .map_err(StorageError::sqlite)?;
        let session_id = format!("qms_{session_random}");
        let transaction = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(StorageError::sqlite)?;
        let now = requested_now.unwrap_or_else(OffsetDateTime::now_utc);
        if now.offset() != UtcOffset::UTC {
            return Err(StorageError::ControlledMeasurementInvalid);
        }
        let (created_at_utc, created_ns) = timestamp(now)?;
        let deadline = now + time::Duration::minutes(i64::from(max_duration_minutes));
        let (deadline_utc, deadline_ns) = timestamp(deadline)?;
        let before = load_snapshot_by_id(&transaction, before_snapshot_id)?
            .ok_or(StorageError::ControlledMeasurementSnapshotMissing)?;
        let before_at = parse_time(&before.acquired_at)?;
        if before_at > now || now - before_at > time::Duration::seconds(BASELINE_FRESHNESS_SECONDS)
        {
            return Err(StorageError::ControlledMeasurementBaselineStale);
        }
        transaction.execute(
            "INSERT INTO controlled_quota_measurement_sessions (session_id, format_version, before_snapshot_id, before_snapshot_sha256, created_at_utc, created_at_unix_ns, deadline_utc, deadline_unix_ns, max_duration_minutes, state, revision, source, evidence_scope, task_attribution) VALUES (?1, 1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'open', 1, ?9, 'account_level', 'not_established')",
            params![session_id, before_snapshot_id, before.payload_sha256, created_at_utc, created_ns, deadline_utc, deadline_ns, max_duration_minutes, SESSION_SOURCE],
        ).map_err(|error| if is_unique_conflict(&error) { StorageError::ControlledMeasurementConflict } else { StorageError::sqlite(error) })?;
        let stored = load_session(&transaction, &session_id)?
            .ok_or(StorageError::ControlledMeasurementCorrupt)?;
        transaction.commit().map_err(StorageError::sqlite)?;
        Ok(stored)
    }

    pub fn finish_controlled_measurement(
        &mut self,
        session_id: &str,
        after_snapshot_id: &str,
        activity: ActivityDeclaration,
    ) -> Result<ControlledMeasurementSession, StorageError> {
        self.finish_controlled_measurement_inner(session_id, after_snapshot_id, activity, None)
    }

    #[cfg(test)]
    pub(crate) fn finish_controlled_measurement_at(
        &mut self,
        session_id: &str,
        after_snapshot_id: &str,
        activity: ActivityDeclaration,
        now: OffsetDateTime,
    ) -> Result<ControlledMeasurementSession, StorageError> {
        self.finish_controlled_measurement_inner(session_id, after_snapshot_id, activity, Some(now))
    }

    fn finish_controlled_measurement_inner(
        &mut self,
        session_id: &str,
        after_snapshot_id: &str,
        activity: ActivityDeclaration,
        requested_now: Option<OffsetDateTime>,
    ) -> Result<ControlledMeasurementSession, StorageError> {
        validate_session_id(session_id)?;
        validate_snapshot_id(after_snapshot_id)
            .map_err(|_| StorageError::ControlledMeasurementInvalid)?;
        let transaction = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(StorageError::sqlite)?;
        let now = requested_now.unwrap_or_else(OffsetDateTime::now_utc);
        if now.offset() != UtcOffset::UTC {
            return Err(StorageError::ControlledMeasurementInvalid);
        }
        let (completed_at, completed_ns) = timestamp(now)?;
        let session = load_session(&transaction, session_id)?
            .ok_or(StorageError::ControlledMeasurementNotFound)?;
        if session.state == MeasurementSessionState::Completed {
            if session.after_snapshot_id.as_deref() == Some(after_snapshot_id)
                && session.activity_declaration == Some(activity)
            {
                let before = load_snapshot_by_id(&transaction, &session.before_snapshot_id)?
                    .ok_or(StorageError::ControlledMeasurementCorrupt)?;
                let after = load_snapshot_by_id(&transaction, after_snapshot_id)?
                    .ok_or(StorageError::ControlledMeasurementCorrupt)?;
                if before.payload_sha256 != session.before_snapshot_sha256
                    || Some(after.payload_sha256.as_str())
                        != session.after_snapshot_sha256.as_deref()
                {
                    return Err(StorageError::ControlledMeasurementCorrupt);
                }
                evaluate_live_quota_comparison(&before, &after)
                    .map_err(|_| StorageError::ControlledMeasurementCorrupt)?;
                transaction.commit().map_err(StorageError::sqlite)?;
                return Ok(session);
            }
            return Err(StorageError::ControlledMeasurementConflict);
        }
        if session.state != MeasurementSessionState::Open {
            return Err(StorageError::ControlledMeasurementTerminal);
        }
        let deadline = parse_time(&session.deadline_utc)?;
        if now > deadline {
            transaction.execute("UPDATE controlled_quota_measurement_sessions SET state = 'expired', completed_at_utc = ?1, completed_at_unix_ns = ?2, revision = revision + 1 WHERE session_id = ?3 AND revision = ?4 AND state = 'open'", params![completed_at, completed_ns, session_id, i64::try_from(session.revision).map_err(|_| StorageError::ControlledMeasurementCorrupt)?]).map_err(StorageError::sqlite)?;
            transaction.commit().map_err(StorageError::sqlite)?;
            return Err(StorageError::ControlledMeasurementExpired);
        }
        let before = load_snapshot_by_id(&transaction, &session.before_snapshot_id)?
            .ok_or(StorageError::ControlledMeasurementCorrupt)?;
        let after = load_snapshot_by_id(&transaction, after_snapshot_id)?
            .ok_or(StorageError::ControlledMeasurementSnapshotMissing)?;
        if before.payload_sha256 != session.before_snapshot_sha256 {
            return Err(StorageError::ControlledMeasurementCorrupt);
        }
        let before_at = parse_time(&before.acquired_at)?;
        let after_at = parse_time(&after.acquired_at)?;
        if after_snapshot_id == session.before_snapshot_id
            || after_at <= before_at
            || after_at < parse_time(&session.created_at_utc)?
            || after_at > deadline
            || after_at > now
        {
            return Err(StorageError::ControlledMeasurementTimingInvalid);
        }
        evaluate_live_quota_comparison(&before, &after)
            .map_err(|_| StorageError::ControlledMeasurementTimingInvalid)?;
        let receipt = TerminalReceipt {
            receipt_version: 1,
            session_id,
            lifecycle_state: MeasurementSessionState::Completed,
            before_snapshot_id: &session.before_snapshot_id,
            before_snapshot_sha256: &before.payload_sha256,
            after_snapshot_id,
            after_snapshot_sha256: &after.payload_sha256,
            policy_version: CONTROLLED_MEASUREMENT_FORMAT_VERSION,
            max_duration_minutes: session.max_duration_minutes,
            activity_declaration: activity,
            completed_at_utc: &completed_at,
            evaluator_contract_version: EVALUATOR_CONTRACT_VERSION,
            source: SESSION_SOURCE,
            evidence_scope: "account_level",
            task_attribution: "not_established",
        };
        let receipt_sha256 = sha256_json(&receipt)?;
        let changed = transaction.execute(
            "UPDATE controlled_quota_measurement_sessions SET state = 'completed', after_snapshot_id = ?1, after_snapshot_sha256 = ?2, completed_at_utc = ?3, completed_at_unix_ns = ?4, activity_declaration = ?5, receipt_sha256 = ?6, revision = revision + 1 WHERE session_id = ?7 AND state = 'open' AND revision = ?8",
            params![after_snapshot_id, after.payload_sha256, completed_at, completed_ns, activity.as_str(), receipt_sha256, session_id, i64::try_from(session.revision).map_err(|_| StorageError::ControlledMeasurementCorrupt)?],
        ).map_err(StorageError::sqlite)?;
        if changed != 1 {
            return Err(StorageError::ControlledMeasurementConflict);
        }
        let stored = load_session(&transaction, session_id)?
            .ok_or(StorageError::ControlledMeasurementCorrupt)?;
        transaction.commit().map_err(StorageError::sqlite)?;
        Ok(stored)
    }

    pub fn abort_controlled_measurement(
        &mut self,
        session_id: &str,
    ) -> Result<ControlledMeasurementSession, StorageError> {
        self.abort_controlled_measurement_inner(session_id, None)
    }

    #[cfg(test)]
    pub(crate) fn abort_controlled_measurement_at(
        &mut self,
        session_id: &str,
        now: OffsetDateTime,
    ) -> Result<ControlledMeasurementSession, StorageError> {
        self.abort_controlled_measurement_inner(session_id, Some(now))
    }

    fn abort_controlled_measurement_inner(
        &mut self,
        session_id: &str,
        requested_now: Option<OffsetDateTime>,
    ) -> Result<ControlledMeasurementSession, StorageError> {
        validate_session_id(session_id)?;
        let transaction = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(StorageError::sqlite)?;
        let now = requested_now.unwrap_or_else(OffsetDateTime::now_utc);
        if now.offset() != UtcOffset::UTC {
            return Err(StorageError::ControlledMeasurementInvalid);
        }
        let (at, ns) = timestamp(now)?;
        let session = load_session(&transaction, session_id)?
            .ok_or(StorageError::ControlledMeasurementNotFound)?;
        if matches!(
            session.state,
            MeasurementSessionState::Aborted | MeasurementSessionState::Expired
        ) {
            transaction.commit().map_err(StorageError::sqlite)?;
            return Ok(session);
        }
        if session.state != MeasurementSessionState::Open {
            return Err(StorageError::ControlledMeasurementTerminal);
        }
        let state = if now > parse_time(&session.deadline_utc)? {
            "expired"
        } else {
            "aborted"
        };
        let changed = transaction.execute("UPDATE controlled_quota_measurement_sessions SET state = ?1, completed_at_utc = ?2, completed_at_unix_ns = ?3, revision = revision + 1 WHERE session_id = ?4 AND state = 'open' AND revision = ?5", params![state, at, ns, session_id, i64::try_from(session.revision).map_err(|_| StorageError::ControlledMeasurementCorrupt)?]).map_err(StorageError::sqlite)?;
        if changed != 1 {
            return Err(StorageError::ControlledMeasurementConflict);
        }
        let stored = load_session(&transaction, session_id)?
            .ok_or(StorageError::ControlledMeasurementCorrupt)?;
        transaction.commit().map_err(StorageError::sqlite)?;
        Ok(stored)
    }
}

impl SqliteReadStore {
    pub fn controlled_measurement_session(
        &mut self,
        session_id: &str,
    ) -> Result<Option<ControlledMeasurementSession>, StorageError> {
        validate_session_id(session_id)?;
        if self.info().latest_applied_migration < 8 {
            return Ok(None);
        }
        load_session(&self.connection, session_id)
    }

    pub fn controlled_measurement_snapshots(
        &mut self,
        session: &ControlledMeasurementSession,
    ) -> Result<
        Option<(
            crate::storage::StoredLiveAccountSnapshot,
            crate::storage::StoredLiveAccountSnapshot,
        )>,
        StorageError,
    > {
        if session.state != MeasurementSessionState::Completed {
            return Ok(None);
        }
        let before = self
            .live_account_snapshot(&session.before_snapshot_id)?
            .ok_or(StorageError::ControlledMeasurementCorrupt)?;
        let after_id = session
            .after_snapshot_id
            .as_deref()
            .ok_or(StorageError::ControlledMeasurementCorrupt)?;
        let after = self
            .live_account_snapshot(after_id)?
            .ok_or(StorageError::ControlledMeasurementCorrupt)?;
        if before.payload_sha256 != session.before_snapshot_sha256
            || Some(after.payload_sha256.clone()) != session.after_snapshot_sha256
        {
            return Err(StorageError::ControlledMeasurementCorrupt);
        }
        Ok(Some((before, after)))
    }
}

fn load_session(
    connection: &Connection,
    id: &str,
) -> Result<Option<ControlledMeasurementSession>, StorageError> {
    let persisted_times: Option<(i64, i64, Option<i64>)> = connection
        .query_row(
            "SELECT created_at_unix_ns, deadline_unix_ns, completed_at_unix_ns FROM controlled_quota_measurement_sessions WHERE session_id = ?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(StorageError::sqlite)?;
    let raw = connection.query_row(
        "SELECT session_id, format_version, before_snapshot_id, before_snapshot_sha256, created_at_utc, deadline_utc, max_duration_minutes, state, after_snapshot_id, after_snapshot_sha256, completed_at_utc, activity_declaration, revision, receipt_sha256, source, evidence_scope, task_attribution FROM controlled_quota_measurement_sessions WHERE session_id = ?1",
        [id],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, String>(2)?, row.get::<_, String>(3)?, row.get::<_, String>(4)?, row.get::<_, String>(5)?, row.get::<_, i64>(6)?, row.get::<_, String>(7)?, row.get::<_, Option<String>>(8)?, row.get::<_, Option<String>>(9)?, row.get::<_, Option<String>>(10)?, row.get::<_, Option<String>>(11)?, row.get::<_, i64>(12)?, row.get::<_, Option<String>>(13)?, row.get::<_, String>(14)?, row.get::<_, String>(15)?, row.get::<_, String>(16)?)),
    ).optional().map_err(StorageError::sqlite)?;
    let Some((
        session_id,
        format,
        before_id,
        before_sha,
        created,
        deadline,
        minutes,
        state,
        after_id,
        after_sha,
        completed,
        activity,
        revision,
        receipt,
        source,
        scope,
        attribution,
    )) = raw
    else {
        return Ok(None);
    };
    let (created_ns, deadline_ns, completed_ns) =
        persisted_times.ok_or(StorageError::ControlledMeasurementCorrupt)?;
    let state = match state.as_str() {
        "open" => MeasurementSessionState::Open,
        "completed" => MeasurementSessionState::Completed,
        "aborted" => MeasurementSessionState::Aborted,
        "expired" => MeasurementSessionState::Expired,
        _ => return Err(StorageError::ControlledMeasurementCorrupt),
    };
    if format != 1
        || source != SESSION_SOURCE
        || scope != "account_level"
        || attribution != "not_established"
        || !(1..=240).contains(&minutes)
        || revision < 1
        || validate_snapshot_id(&before_id).is_err()
        || !valid_hash(&before_sha)
    {
        return Err(StorageError::ControlledMeasurementCorrupt);
    }
    let created_time =
        parse_time(&created).map_err(|_| StorageError::ControlledMeasurementCorrupt)?;
    let deadline_time =
        parse_time(&deadline).map_err(|_| StorageError::ControlledMeasurementCorrupt)?;
    if timestamp(created_time)?.0 != created
        || created_time.unix_timestamp_nanos() != i128::from(created_ns)
        || timestamp(deadline_time)?.0 != deadline
        || deadline_time.unix_timestamp_nanos() != i128::from(deadline_ns)
        || deadline_time - created_time != time::Duration::minutes(minutes)
    {
        return Err(StorageError::ControlledMeasurementCorrupt);
    }
    if state == MeasurementSessionState::Completed {
        let after_id = after_id
            .as_deref()
            .ok_or(StorageError::ControlledMeasurementCorrupt)?;
        let after_sha = after_sha
            .as_deref()
            .ok_or(StorageError::ControlledMeasurementCorrupt)?;
        let completed_at = completed
            .as_deref()
            .ok_or(StorageError::ControlledMeasurementCorrupt)?;
        let activity = ActivityDeclaration::parse(
            activity
                .as_deref()
                .ok_or(StorageError::ControlledMeasurementCorrupt)?,
        )?;
        if !valid_hash(after_sha)
            || validate_snapshot_id(after_id).is_err()
            || !valid_hash(
                receipt
                    .as_deref()
                    .ok_or(StorageError::ControlledMeasurementCorrupt)?,
            )
        {
            return Err(StorageError::ControlledMeasurementCorrupt);
        }
        let completed_time =
            parse_time(completed_at).map_err(|_| StorageError::ControlledMeasurementCorrupt)?;
        if timestamp(completed_time)?.0 != completed_at
            || Some(completed_time.unix_timestamp_nanos()) != completed_ns.map(i128::from)
            || completed_time < created_time
            || completed_time > deadline_time
        {
            return Err(StorageError::ControlledMeasurementCorrupt);
        }
        let before = load_snapshot_by_id(connection, &before_id)?
            .ok_or(StorageError::ControlledMeasurementCorrupt)?;
        let after = load_snapshot_by_id(connection, after_id)?
            .ok_or(StorageError::ControlledMeasurementCorrupt)?;
        if before.payload_sha256 != before_sha || after.payload_sha256 != after_sha {
            return Err(StorageError::ControlledMeasurementCorrupt);
        }
        let before_time = parse_time(&before.acquired_at)?;
        let after_time = parse_time(&after.acquired_at)?;
        if before_time > created_time
            || created_time - before_time > time::Duration::seconds(BASELINE_FRESHNESS_SECONDS)
            || after_time < created_time
            || after_time > deadline_time
            || after_time > completed_time
        {
            return Err(StorageError::ControlledMeasurementCorrupt);
        }
        evaluate_live_quota_comparison(&before, &after)
            .map_err(|_| StorageError::ControlledMeasurementCorrupt)?;
        let expected = sha256_json(&TerminalReceipt {
            receipt_version: 1,
            session_id: &session_id,
            lifecycle_state: state,
            before_snapshot_id: &before_id,
            before_snapshot_sha256: &before_sha,
            after_snapshot_id: after_id,
            after_snapshot_sha256: after_sha,
            policy_version: 1,
            max_duration_minutes: minutes as u16,
            activity_declaration: activity,
            completed_at_utc: completed_at,
            evaluator_contract_version: EVALUATOR_CONTRACT_VERSION,
            source: SESSION_SOURCE,
            evidence_scope: "account_level",
            task_attribution: "not_established",
        })?;
        if receipt != Some(expected) {
            return Err(StorageError::ControlledMeasurementCorrupt);
        }
    } else {
        if after_id.is_some() || after_sha.is_some() || receipt.is_some() || activity.is_some() {
            return Err(StorageError::ControlledMeasurementCorrupt);
        }
        let before = load_snapshot_by_id(connection, &before_id)?
            .ok_or(StorageError::ControlledMeasurementCorrupt)?;
        if before.payload_sha256 != before_sha {
            return Err(StorageError::ControlledMeasurementCorrupt);
        }
        let before_time = parse_time(&before.acquired_at)?;
        if before_time > created_time
            || created_time - before_time > time::Duration::seconds(BASELINE_FRESHNESS_SECONDS)
        {
            return Err(StorageError::ControlledMeasurementCorrupt);
        }
        if let Some(completed_at) = completed.as_deref() {
            if state == MeasurementSessionState::Open {
                return Err(StorageError::ControlledMeasurementCorrupt);
            }
            let completed_time =
                parse_time(completed_at).map_err(|_| StorageError::ControlledMeasurementCorrupt)?;
            if timestamp(completed_time)?.0 != completed_at
                || Some(completed_time.unix_timestamp_nanos()) != completed_ns.map(i128::from)
                || completed_time < created_time
                || (state == MeasurementSessionState::Aborted && completed_time > deadline_time)
                || (state == MeasurementSessionState::Expired && completed_time <= deadline_time)
            {
                return Err(StorageError::ControlledMeasurementCorrupt);
            }
        } else if state != MeasurementSessionState::Open || completed_ns.is_some() {
            return Err(StorageError::ControlledMeasurementCorrupt);
        }
    }
    Ok(Some(ControlledMeasurementSession {
        session_id,
        format_version: format as u32,
        state,
        before_snapshot_id: before_id,
        before_snapshot_sha256: before_sha,
        created_at_utc: created,
        deadline_utc: deadline,
        max_duration_minutes: minutes as u16,
        after_snapshot_id: after_id,
        after_snapshot_sha256: after_sha,
        completed_at_utc: completed,
        activity_declaration: activity
            .as_deref()
            .map(ActivityDeclaration::parse)
            .transpose()?,
        revision: revision as u64,
        receipt_sha256: receipt,
        source: SESSION_SOURCE,
        evidence_scope: "account_level",
        task_attribution: "not_established",
    }))
}

fn validate_session_id(id: &str) -> Result<(), StorageError> {
    if id.len() == 36
        && id.starts_with("qms_")
        && id[4..]
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        Ok(())
    } else {
        Err(StorageError::ControlledMeasurementInvalid)
    }
}
fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn timestamp(value: OffsetDateTime) -> Result<(String, i64), StorageError> {
    let value = value.to_offset(UtcOffset::UTC);
    let text = value
        .format(&Rfc3339)
        .map_err(|_| StorageError::ControlledMeasurementInvalid)?;
    let ns = i64::try_from(value.unix_timestamp_nanos())
        .map_err(|_| StorageError::ControlledMeasurementInvalid)?;
    Ok((text, ns))
}
fn parse_time(value: &str) -> Result<OffsetDateTime, StorageError> {
    let result = OffsetDateTime::parse(value, &Rfc3339)
        .map_err(|_| StorageError::ControlledMeasurementCorrupt)?;
    if result.offset() != UtcOffset::UTC {
        return Err(StorageError::ControlledMeasurementCorrupt);
    }
    Ok(result)
}
fn sha256_json(value: &impl Serialize) -> Result<String, StorageError> {
    let bytes =
        serde_json::to_vec(value).map_err(|_| StorageError::ControlledMeasurementInvalid)?;
    Ok(Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}
fn is_unique_conflict(error: &rusqlite::Error) -> bool {
    matches!(error, rusqlite::Error::SqliteFailure(e, _) if e.code == rusqlite::ErrorCode::ConstraintViolation)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codex_app_server::{
        AccountRateLimitsSnapshot, BucketIdentity, MeterAvailability, MeterSnapshot, QuotaWarning,
        SnapshotAvailability,
    };
    use crate::storage::LIVE_ACCOUNT_QUOTA_PROVENANCE;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_PATH: AtomicU64 = AtomicU64::new(0);
    fn database() -> PathBuf {
        std::env::temp_dir().join(format!(
            "codex-meter-p046b2-{}-{}.sqlite",
            std::process::id(),
            NEXT_PATH.fetch_add(1, Ordering::Relaxed)
        ))
    }
    fn cleanup(path: &std::path::Path) {
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
        let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
    }
    fn snapshot(at: &str, used: f64) -> AccountRateLimitsSnapshot {
        let meter = |percent: f64, reset: &str| MeterSnapshot {
            availability: MeterAvailability::Available,
            used_percent: Some(percent),
            used_percent_provenance: "observed",
            remaining_percent: Some(100.0 - percent),
            remaining_percent_provenance: "derived",
            reset_at_utc: Some(reset.to_owned()),
        };
        AccountRateLimitsSnapshot {
            schema_version: "1.0.0",
            source: crate::storage::LIVE_ACCOUNT_QUOTA_SOURCE,
            acquired_at: at.to_owned(),
            availability: SnapshotAvailability::Available,
            bucket_identity: BucketIdentity::MainCodex,
            five_hour: meter(used, "2026-10-01T05:00:00Z"),
            weekly: meter(used, "2026-10-08T00:00:00Z"),
            warnings: Vec::<QuotaWarning>::new(),
            provenance: LIVE_ACCOUNT_QUOTA_PROVENANCE,
        }
    }
    fn at(value: &str) -> OffsetDateTime {
        OffsetDateTime::parse(value, &Rfc3339).unwrap()
    }
    fn snapshots(store: &mut SqliteStore) -> (String, String) {
        let before = store
            .save_live_account_snapshot(&snapshot("2026-10-01T00:00:00Z", 10.0))
            .unwrap();
        let after = store
            .save_live_account_snapshot(&snapshot("2026-10-01T00:01:00Z", 12.0))
            .unwrap();
        (before.snapshot.snapshot_id, after.snapshot.snapshot_id)
    }

    #[test]
    fn file_backed_finish_is_atomic_replay_safe_and_receipt_is_verified_after_restart() {
        let path = database();
        let (before, after, session_id, original_receipt) = {
            let mut store = SqliteStore::open(&path).unwrap();
            assert!(store.foreign_keys_enabled().unwrap());
            let (before, after) = snapshots(&mut store);
            let session = store
                .start_controlled_measurement_at(&before, 60, at("2026-10-01T00:00:30Z"))
                .unwrap();
            assert_eq!(session.revision, 1);
            assert_eq!(
                store
                    .start_controlled_measurement_at(&before, 60, at("2026-10-01T00:00:31Z"))
                    .unwrap_err()
                    .code(),
                "measurement_conflict"
            );
            let completed = store
                .finish_controlled_measurement_at(
                    &session.session_id,
                    &after,
                    ActivityDeclaration::SelfReportedNoKnownOverlap,
                    at("2026-10-01T00:02:00Z"),
                )
                .unwrap();
            assert_eq!(completed.state, MeasurementSessionState::Completed);
            assert_eq!(completed.revision, 2);
            assert!(store.connection.execute("UPDATE controlled_quota_measurement_sessions SET receipt_sha256 = ?1 WHERE session_id = ?2", params!["0".repeat(64), session.session_id]).is_err());
            assert!(store
                .connection
                .execute(
                    "DELETE FROM controlled_quota_measurement_sessions WHERE session_id = ?1",
                    [&session.session_id]
                )
                .is_err());
            assert!(store
                .connection
                .execute(
                    "DELETE FROM live_account_quota_snapshots WHERE snapshot_id = ?1",
                    [&before]
                )
                .is_err());
            let replay = store
                .finish_controlled_measurement_at(
                    &session.session_id,
                    &after,
                    ActivityDeclaration::SelfReportedNoKnownOverlap,
                    at("2026-10-01T00:03:00Z"),
                )
                .unwrap();
            assert_eq!(replay, completed);
            assert_eq!(
                store
                    .finish_controlled_measurement_at(
                        &session.session_id,
                        &before,
                        ActivityDeclaration::Unknown,
                        at("2026-10-01T00:03:00Z")
                    )
                    .unwrap_err()
                    .code(),
                "measurement_conflict"
            );
            (
                before,
                after,
                session.session_id,
                completed.receipt_sha256.unwrap(),
            )
        };
        let mut read = SqliteReadStore::open(&path).unwrap();
        let recovered = read
            .controlled_measurement_session(&session_id)
            .unwrap()
            .unwrap();
        assert_eq!(recovered.state, MeasurementSessionState::Completed);
        assert_eq!(recovered.revision, 2);
        assert_eq!(recovered.before_snapshot_id, before);
        assert_eq!(recovered.after_snapshot_id.as_deref(), Some(after.as_str()));
        assert_eq!(
            recovered.receipt_sha256.as_deref(),
            Some(original_receipt.as_str())
        );
        let snapshots = read
            .controlled_measurement_snapshots(&recovered)
            .unwrap()
            .unwrap();
        assert_eq!(snapshots.0.snapshot_id, before);
        assert_eq!(snapshots.1.snapshot_id, after);
        drop(read);
        cleanup(&path);
    }

    #[test]
    fn invalid_timing_rolls_back_and_abort_survives_restart() {
        let path = database();
        let session_id;
        {
            let mut store = SqliteStore::open(&path).unwrap();
            let (before, _) = snapshots(&mut store);
            let session = store
                .start_controlled_measurement_at(&before, 60, at("2026-10-01T00:00:30Z"))
                .unwrap();
            session_id = session.session_id.clone();
            assert!(store.connection.execute("UPDATE controlled_quota_measurement_sessions SET max_duration_minutes = 30 WHERE session_id = ?1", [&session_id]).is_err());
            assert_eq!(
                store
                    .finish_controlled_measurement_at(
                        &session_id,
                        &before,
                        ActivityDeclaration::Unknown,
                        at("2026-10-01T00:02:00Z")
                    )
                    .unwrap_err()
                    .code(),
                "measurement_timing_invalid"
            );
            let unchanged: i64 = store.connection.query_row("SELECT revision FROM controlled_quota_measurement_sessions WHERE session_id = ?1", [&session_id], |row| row.get(0)).unwrap();
            assert_eq!(unchanged, 1);
            let aborted = store
                .abort_controlled_measurement_at(&session_id, at("2026-10-01T00:03:00Z"))
                .unwrap();
            assert_eq!(aborted.state, MeasurementSessionState::Aborted);
            assert_eq!(aborted.revision, 2);
        }
        let mut store = SqliteStore::open(&path).unwrap();
        let recovered = store
            .abort_controlled_measurement_at(&session_id, at("2026-10-01T00:04:00Z"))
            .unwrap();
        assert_eq!(recovered.state, MeasurementSessionState::Aborted);
        assert_eq!(
            store
                .finish_controlled_measurement_at(
                    &session_id,
                    "lqs_00000000000000000000000000000000",
                    ActivityDeclaration::Unknown,
                    at("2026-10-01T00:03:00Z")
                )
                .unwrap_err()
                .code(),
            "measurement_conflict"
        );
        cleanup(&path);
    }

    #[test]
    fn expired_finish_closes_session_and_cannot_be_replayed_as_completed() {
        let path = database();
        let mut store = SqliteStore::open(&path).unwrap();
        let (before, after) = snapshots(&mut store);
        let started = store
            .start_controlled_measurement_at(&before, 1, at("2026-10-01T00:00:30Z"))
            .unwrap();
        assert_eq!(
            store
                .finish_controlled_measurement_at(
                    &started.session_id,
                    &after,
                    ActivityDeclaration::Unknown,
                    at("2026-10-01T00:02:00Z")
                )
                .unwrap_err()
                .code(),
            "measurement_expired"
        );
        drop(store);
        let mut read = SqliteReadStore::open(&path).unwrap();
        let expired = read
            .controlled_measurement_session(&started.session_id)
            .unwrap()
            .unwrap();
        assert_eq!(expired.state, MeasurementSessionState::Expired);
        assert_eq!(expired.revision, 2);
        assert!(expired.receipt_sha256.is_none());
        assert!(expired.after_snapshot_id.is_none());
        drop(read);
        let mut store = SqliteStore::open(&path).unwrap();
        for (snapshot_id, activity) in [
            (&after, ActivityDeclaration::Unknown),
            (&before, ActivityDeclaration::Unknown),
        ] {
            assert_eq!(
                store
                    .finish_controlled_measurement_at(
                        &started.session_id,
                        snapshot_id,
                        activity,
                        at("2026-10-01T00:02:01Z")
                    )
                    .unwrap_err()
                    .code(),
                "measurement_conflict"
            );
        }
        let replacement = store
            .start_controlled_measurement_at(&after, 1, at("2026-10-01T00:02:01Z"))
            .unwrap();
        assert_eq!(replacement.state, MeasurementSessionState::Open);
        drop(store);
        let mut read = SqliteReadStore::open(&path).unwrap();
        let unchanged = read
            .controlled_measurement_session(&started.session_id)
            .unwrap()
            .unwrap();
        assert_eq!(unchanged.state, MeasurementSessionState::Expired);
        assert_eq!(unchanged.revision, 2);
        assert!(unchanged.receipt_sha256.is_none());
        drop(read);
        cleanup(&path);
    }

    #[test]
    fn concurrent_starts_and_conflicting_finishes_have_one_winner() {
        use std::sync::{Arc, Barrier};
        let path = database();
        let before;
        {
            let mut store = SqliteStore::open(&path).unwrap();
            before = snapshots(&mut store).0;
        }
        let barrier = Arc::new(Barrier::new(3));
        let starts: Vec<_> = (0..2)
            .map(|_| {
                let path = path.clone();
                let before = before.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let mut store = SqliteStore::open(path).unwrap();
                    barrier.wait();
                    store.start_controlled_measurement_at(&before, 60, at("2026-10-01T00:00:30Z"))
                })
            })
            .collect();
        barrier.wait();
        let results: Vec<_> = starts
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        let session = results.into_iter().find_map(Result::ok).unwrap();
        let (after_one, after_two) = {
            let mut store = SqliteStore::open(&path).unwrap();
            (
                store
                    .save_live_account_snapshot(&snapshot("2026-10-01T00:01:00Z", 12.0))
                    .unwrap()
                    .snapshot
                    .snapshot_id,
                store
                    .save_live_account_snapshot(&snapshot("2026-10-01T00:02:00Z", 14.0))
                    .unwrap()
                    .snapshot
                    .snapshot_id,
            )
        };
        let barrier = Arc::new(Barrier::new(3));
        let finishes: Vec<_> = [after_one, after_two]
            .into_iter()
            .map(|after| {
                let path = path.clone();
                let id = session.session_id.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let mut store = SqliteStore::open(path).unwrap();
                    barrier.wait();
                    store
                        .finish_controlled_measurement_at(
                            &id,
                            &after,
                            ActivityDeclaration::Unknown,
                            at("2026-10-01T00:03:00Z"),
                        )
                        .map(|row| row.after_snapshot_id.unwrap())
                })
            })
            .collect();
        barrier.wait();
        let outcomes: Vec<_> = finishes
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect();
        assert_eq!(outcomes.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(outcomes.iter().filter(|result| result.is_err()).count(), 1);
        let mut read = SqliteReadStore::open(&path).unwrap();
        let stored = read
            .controlled_measurement_session(&session.session_id)
            .unwrap()
            .unwrap();
        assert_eq!(stored.state, MeasurementSessionState::Completed);
        assert!(outcomes
            .iter()
            .filter_map(|result| result.as_ref().ok())
            .any(|id| Some(id) == stored.after_snapshot_id.as_ref()));
        cleanup(&path);
    }

    #[test]
    fn migration_seven_upgrade_preserves_rows_and_corrupt_receipt_is_rejected() {
        let path = database();
        let migrations = &super::super::migrations::MIGRATIONS[..7];
        let preserved_snapshot_id;
        {
            let mut store =
                super::super::sqlite::open_with_migrations(&path, false, migrations).unwrap();
            store
                .connection
                .execute(
                    "INSERT INTO storage_metadata (key, value) VALUES ('preserved', 'yes')",
                    [],
                )
                .unwrap();
            preserved_snapshot_id = store
                .save_live_account_snapshot(&snapshot("2026-10-01T00:00:00Z", 10.0))
                .unwrap()
                .snapshot
                .snapshot_id;
        }
        {
            let mut store = SqliteStore::open(&path).unwrap();
            assert_eq!(store.info().latest_applied_migration, 8);
            let preserved: String = store
                .connection
                .query_row(
                    "SELECT value FROM storage_metadata WHERE key = 'preserved'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(preserved, "yes");
            let (before, after) = snapshots(&mut store);
            let session = store
                .start_controlled_measurement_at(&before, 60, at("2026-10-01T00:00:30Z"))
                .unwrap();
            store
                .finish_controlled_measurement_at(
                    &session.session_id,
                    &after,
                    ActivityDeclaration::KnownLocalOverlap,
                    at("2026-10-01T00:02:00Z"),
                )
                .unwrap();
            store
                .connection
                .execute_batch("DROP TRIGGER controlled_quota_session_receipt_immutable;")
                .unwrap();
            store.connection.execute("UPDATE controlled_quota_measurement_sessions SET receipt_sha256 = ?1 WHERE session_id = ?2", params!["0".repeat(64), session.session_id]).unwrap();
        }
        let mut read = SqliteReadStore::open(&path).unwrap();
        assert_eq!(
            read.live_account_snapshot(&preserved_snapshot_id)
                .unwrap()
                .unwrap()
                .payload_sha256
                .len(),
            64
        );
        let connection = rusqlite::Connection::open_with_flags(
            &path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let id: String = connection
            .query_row(
                "SELECT session_id FROM controlled_quota_measurement_sessions",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            read.controlled_measurement_session(&id).unwrap_err().code(),
            "database_corrupt"
        );
        cleanup(&path);
    }

    #[test]
    fn trigger_failures_roll_back_start_and_finish_and_commit_replay_survives_restart() {
        let path = database();
        let (session_id, before, after) = {
            let mut store = SqliteStore::open(&path).unwrap();
            let (before, after) = snapshots(&mut store);
            store.connection.execute_batch("CREATE TRIGGER fail_measure_start BEFORE INSERT ON controlled_quota_measurement_sessions BEGIN SELECT RAISE(ABORT, 'synthetic start failure'); END;").unwrap();
            assert!(store
                .start_controlled_measurement_at(&before, 60, at("2026-10-01T00:00:30Z"))
                .is_err());
            let count: i64 = store
                .connection
                .query_row(
                    "SELECT count(*) FROM controlled_quota_measurement_sessions",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(count, 0);
            store
                .connection
                .execute_batch("DROP TRIGGER fail_measure_start;")
                .unwrap();
            let session = store
                .start_controlled_measurement_at(&before, 60, at("2026-10-01T00:00:30Z"))
                .unwrap();
            store.connection.execute_batch("CREATE TRIGGER fail_measure_finish BEFORE UPDATE ON controlled_quota_measurement_sessions WHEN NEW.state = 'completed' BEGIN SELECT RAISE(ABORT, 'synthetic finish failure'); END;").unwrap();
            assert!(store
                .finish_controlled_measurement_at(
                    &session.session_id,
                    &after,
                    ActivityDeclaration::KnownLocalOverlap,
                    at("2026-10-01T00:02:00Z")
                )
                .is_err());
            drop(store);
            (session.session_id, before, after)
        };
        let mut reader = SqliteReadStore::open(&path).unwrap();
        let open = reader
            .controlled_measurement_session(&session_id)
            .unwrap()
            .unwrap();
        assert_eq!(open.state, MeasurementSessionState::Open);
        assert_eq!(open.revision, 1);
        drop(reader);
        let completed = {
            let mut store = SqliteStore::open(&path).unwrap();
            store
                .connection
                .execute_batch("DROP TRIGGER fail_measure_finish;")
                .unwrap();
            store
                .finish_controlled_measurement_at(
                    &session_id,
                    &after,
                    ActivityDeclaration::KnownLocalOverlap,
                    at("2026-10-01T00:02:00Z"),
                )
                .unwrap()
        };
        let reopened_retry = {
            let mut store = SqliteStore::open(&path).unwrap();
            store
                .finish_controlled_measurement_at(
                    &session_id,
                    &after,
                    ActivityDeclaration::KnownLocalOverlap,
                    at("2026-10-01T00:03:00Z"),
                )
                .unwrap()
        };
        assert_eq!(completed, reopened_retry);
        assert_eq!(reopened_retry.state, MeasurementSessionState::Completed);
        assert_eq!(reopened_retry.revision, 2);
        assert_eq!(reopened_retry.before_snapshot_id, before);
        assert_eq!(completed.receipt_sha256, reopened_retry.receipt_sha256);
        cleanup(&path);
    }
}
