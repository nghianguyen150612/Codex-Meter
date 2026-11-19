//! Separately versioned persistence for explicitly saved account-level quota snapshots.

use std::collections::HashSet;

use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::Serialize;
use sha2::{Digest, Sha256};
use time::{format_description::well_known::Rfc3339, OffsetDateTime, UtcOffset};

use crate::codex_app_server::{
    AccountRateLimitsSnapshot, BucketIdentity, MeterAvailability, MeterSnapshot, QuotaWarning,
    SnapshotAvailability,
};

use super::sqlite::{SqliteReadStore, SqliteStore, StorageError};

pub const LIVE_ACCOUNT_QUOTA_SOURCE: &str = "codex_app_server_account_rate_limits";
pub const LIVE_ACCOUNT_QUOTA_EVIDENCE_SCOPE: &str = "account_level";
pub const LIVE_ACCOUNT_QUOTA_TASK_ATTRIBUTION: &str = "not_established";
pub const LIVE_ACCOUNT_QUOTA_FORECAST_QUALIFICATION: &str = "not_established";
pub const LIVE_ACCOUNT_QUOTA_STORAGE_SCHEMA_VERSION: u32 = 1;

const SNAPSHOT_SCHEMA_VERSION: &str = "1.0.0";
const SNAPSHOT_PROVENANCE: &str =
    "account_level_snapshot; task_attribution_not_established; forecast_qualification_not_established";
const MAX_WARNING_CODES: usize = 32;
const MAX_HISTORY_LIMIT: u32 = 1000;
const SNAPSHOT_ID_PREFIX: &str = "lqs_";
const SNAPSHOT_COLUMNS: &str = "snapshot_id, acquired_at_utc, acquired_at_unix_ns, storage_schema_version, source, evidence_scope, task_attribution, forecast_qualification, overall_availability, main_bucket_identity, five_hour_availability, five_hour_used_percent, five_hour_reset_at_utc, weekly_availability, weekly_used_percent, weekly_reset_at_utc, warning_codes, payload_sha256";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveSnapshotSaveDisposition {
    Persisted,
    IdempotentRetry,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StoredLiveQuotaMeter {
    pub availability: MeterAvailability,
    pub used_percent: Option<f64>,
    pub used_percent_provenance: &'static str,
    pub remaining_percent_derived: Option<f64>,
    pub remaining_percent_provenance: &'static str,
    pub reset_at_utc: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StoredLiveAccountSnapshot {
    pub snapshot_id: String,
    pub storage_schema_version: u32,
    pub schema_version: &'static str,
    pub source: &'static str,
    pub acquired_at: String,
    pub availability: SnapshotAvailability,
    pub bucket_identity: BucketIdentity,
    pub five_hour: StoredLiveQuotaMeter,
    pub weekly: StoredLiveQuotaMeter,
    pub warnings: Vec<QuotaWarning>,
    pub evidence_scope: &'static str,
    pub task_attribution: &'static str,
    pub forecast_qualification: &'static str,
    pub provenance: &'static str,
    pub payload_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SavedLiveAccountSnapshot {
    pub persistence: LiveSnapshotSaveDisposition,
    pub snapshot: StoredLiveAccountSnapshot,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LiveAccountSnapshotHistoryPage {
    pub schema_version: &'static str,
    pub source: &'static str,
    pub evidence_scope: &'static str,
    pub task_attribution: &'static str,
    pub forecast_qualification: &'static str,
    pub snapshots: Vec<StoredLiveAccountSnapshot>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug)]
struct RawSnapshot {
    snapshot_id: String,
    acquired_at_utc: String,
    acquired_at_unix_ns: i64,
    storage_schema_version: i64,
    source: String,
    evidence_scope: String,
    task_attribution: String,
    forecast_qualification: String,
    overall_availability: String,
    main_bucket_identity: String,
    five_hour_availability: String,
    five_hour_used_percent: Option<f64>,
    five_hour_reset_at_utc: Option<String>,
    weekly_availability: String,
    weekly_used_percent: Option<f64>,
    weekly_reset_at_utc: Option<String>,
    warning_codes: String,
    payload_sha256: String,
}

#[derive(Serialize)]
struct CanonicalSnapshotPayload<'a> {
    snapshot_id: &'a str,
    storage_schema_version: u32,
    schema_version: &'static str,
    source: &'static str,
    acquired_at: &'a str,
    availability: SnapshotAvailability,
    bucket_identity: BucketIdentity,
    five_hour: &'a StoredLiveQuotaMeter,
    weekly: &'a StoredLiveQuotaMeter,
    warnings: &'a [QuotaWarning],
    evidence_scope: &'static str,
    task_attribution: &'static str,
    forecast_qualification: &'static str,
    provenance: &'static str,
}

impl SqliteStore {
    /// Persist one normalized App Server acquisition under a fresh random event ID.
    /// The ID is generated from SQLite's OS-seeded pseudo-random generator and is
    /// unrelated to account, task, machine, or provider identity.
    pub(crate) fn save_live_account_snapshot(
        &mut self,
        snapshot: &AccountRateLimitsSnapshot,
    ) -> Result<SavedLiveAccountSnapshot, StorageError> {
        let random: String = self
            .connection
            .query_row("SELECT lower(hex(randomblob(16)))", [], |row| row.get(0))
            .map_err(StorageError::sqlite)?;
        let snapshot_id = format!("{SNAPSHOT_ID_PREFIX}{random}");
        self.retry_live_account_snapshot(&snapshot_id, snapshot)
    }

    /// Persistence entry point for one stable attempt identity. Reusing the same
    /// ID and payload is idempotent; a different payload fails closed.
    pub(crate) fn retry_live_account_snapshot(
        &mut self,
        snapshot_id: &str,
        snapshot: &AccountRateLimitsSnapshot,
    ) -> Result<SavedLiveAccountSnapshot, StorageError> {
        self.persist_live_account_snapshot_attempt(snapshot_id, snapshot)
    }

    fn persist_live_account_snapshot_attempt(
        &mut self,
        snapshot_id: &str,
        snapshot: &AccountRateLimitsSnapshot,
    ) -> Result<SavedLiveAccountSnapshot, StorageError> {
        validate_snapshot_id(snapshot_id)?;
        let acquired_at = normalize_utc_timestamp(&snapshot.acquired_at)?;
        let (acquired_at, acquired_at_unix_ns) = acquired_at;
        let normalized = normalize_snapshot(snapshot_id, snapshot, acquired_at)?;
        let payload_sha256 = snapshot_checksum(&normalized)?;
        let mut record = normalized;
        record.payload_sha256 = payload_sha256.clone();

        let transaction = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(StorageError::sqlite)?;
        if let Some(existing) = load_snapshot_by_id(&transaction, snapshot_id)? {
            if existing.payload_sha256 != payload_sha256 {
                return Err(StorageError::LiveAccountSnapshotConflict);
            }
            transaction.commit().map_err(StorageError::sqlite)?;
            return Ok(SavedLiveAccountSnapshot {
                persistence: LiveSnapshotSaveDisposition::IdempotentRetry,
                snapshot: existing,
            });
        }

        let warning_codes = serde_json::to_string(&record.warnings)
            .map_err(|_| StorageError::LiveAccountSnapshotInvalid)?;
        transaction
            .execute(
                "INSERT INTO live_account_quota_snapshots (snapshot_id, acquired_at_utc, acquired_at_unix_ns, storage_schema_version, source, evidence_scope, task_attribution, forecast_qualification, overall_availability, main_bucket_identity, five_hour_availability, five_hour_used_percent, five_hour_reset_at_utc, weekly_availability, weekly_used_percent, weekly_reset_at_utc, warning_codes, payload_sha256) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
                params![
                    record.snapshot_id,
                    record.acquired_at,
                    acquired_at_unix_ns,
                    record.storage_schema_version,
                    record.source,
                    record.evidence_scope,
                    record.task_attribution,
                    record.forecast_qualification,
                    snapshot_availability_code(record.availability),
                    bucket_identity_code(record.bucket_identity),
                    meter_availability_code(record.five_hour.availability),
                    record.five_hour.used_percent,
                    record.five_hour.reset_at_utc,
                    meter_availability_code(record.weekly.availability),
                    record.weekly.used_percent,
                    record.weekly.reset_at_utc,
                    warning_codes,
                    record.payload_sha256,
                ],
            )
            .map_err(StorageError::sqlite)?;
        transaction.commit().map_err(StorageError::sqlite)?;

        Ok(SavedLiveAccountSnapshot {
            persistence: LiveSnapshotSaveDisposition::Persisted,
            snapshot: record,
        })
    }
}

impl SqliteReadStore {
    /// Read only the dedicated P045B live account snapshot table.
    pub fn live_account_snapshot_history(
        &mut self,
        limit: u32,
        after: Option<&str>,
    ) -> Result<LiveAccountSnapshotHistoryPage, StorageError> {
        if limit == 0 || limit > MAX_HISTORY_LIMIT {
            return Err(StorageError::LiveAccountSnapshotCursorInvalid);
        }
        let mut snapshots = Vec::new();
        let mut next_cursor = None;
        if self.info().latest_applied_migration >= 7 {
            let cursor_key = match after {
                Some(snapshot_id) => {
                    validate_snapshot_id(snapshot_id)
                        .map_err(|_| StorageError::LiveAccountSnapshotCursorInvalid)?;
                    let record = load_snapshot_by_id(&self.connection, snapshot_id)?
                        .ok_or(StorageError::LiveAccountSnapshotCursorInvalid)?;
                    let (_, acquired_at_unix_ns) = normalize_utc_timestamp(&record.acquired_at)
                        .map_err(|_| StorageError::LiveAccountSnapshotCorrupt)?;
                    Some((acquired_at_unix_ns, snapshot_id.to_owned()))
                }
                None => None,
            };
            let fetch_limit = i64::from(limit) + 1;
            let mut statement = self.connection.prepare(&format!(
                "SELECT {SNAPSHOT_COLUMNS} FROM live_account_quota_snapshots WHERE (?1 IS NULL OR acquired_at_unix_ns < ?2 OR (acquired_at_unix_ns = ?2 AND snapshot_id < ?3)) ORDER BY acquired_at_unix_ns DESC, snapshot_id DESC LIMIT ?4"
            )).map_err(StorageError::sqlite)?;
            let (cursor_ns, cursor_id) = cursor_key
                .map(|(timestamp, id)| (Some(timestamp), Some(id)))
                .unwrap_or((None, None));
            let rows = statement
                .query_map(
                    params![cursor_ns, cursor_ns, cursor_id, fetch_limit],
                    read_raw_snapshot,
                )
                .map_err(StorageError::sqlite)?;
            for row in rows {
                let raw = row.map_err(|_| StorageError::LiveAccountSnapshotCorrupt)?;
                let record = decode_snapshot(raw)?;
                if snapshots.len() == limit as usize {
                    next_cursor = snapshots
                        .last()
                        .map(|snapshot: &StoredLiveAccountSnapshot| snapshot.snapshot_id.clone());
                    break;
                }
                snapshots.push(record);
            }
        } else if after.is_some() {
            return Err(StorageError::LiveAccountSnapshotCursorInvalid);
        }

        Ok(LiveAccountSnapshotHistoryPage {
            schema_version: "1.0.0",
            source: LIVE_ACCOUNT_QUOTA_SOURCE,
            evidence_scope: LIVE_ACCOUNT_QUOTA_EVIDENCE_SCOPE,
            task_attribution: LIVE_ACCOUNT_QUOTA_TASK_ATTRIBUTION,
            forecast_qualification: LIVE_ACCOUNT_QUOTA_FORECAST_QUALIFICATION,
            snapshots,
            next_cursor,
        })
    }
}

fn normalize_snapshot(
    snapshot_id: &str,
    snapshot: &AccountRateLimitsSnapshot,
    acquired_at: String,
) -> Result<StoredLiveAccountSnapshot, StorageError> {
    if snapshot.schema_version != SNAPSHOT_SCHEMA_VERSION
        || snapshot.source != LIVE_ACCOUNT_QUOTA_SOURCE
        || snapshot.provenance != SNAPSHOT_PROVENANCE
        || snapshot.warnings.len() > MAX_WARNING_CODES
    {
        return Err(StorageError::LiveAccountSnapshotInvalid);
    }
    let mut unique_warnings = HashSet::new();
    let warnings = snapshot
        .warnings
        .iter()
        .copied()
        .filter(|warning| unique_warnings.insert(*warning))
        .collect::<Vec<_>>();

    let five_hour = normalize_meter(&snapshot.five_hour)?;
    let weekly = normalize_meter(&snapshot.weekly)?;
    let expected_availability = match (five_hour.availability, weekly.availability) {
        (MeterAvailability::Ambiguous, _) | (_, MeterAvailability::Ambiguous) => {
            SnapshotAvailability::Ambiguous
        }
        (MeterAvailability::Available, MeterAvailability::Available) => {
            SnapshotAvailability::Available
        }
        (MeterAvailability::Unavailable, MeterAvailability::Unavailable) => {
            SnapshotAvailability::Unavailable
        }
        _ => SnapshotAvailability::Partial,
    };
    if snapshot.availability != expected_availability {
        return Err(StorageError::LiveAccountSnapshotInvalid);
    }
    if snapshot.bucket_identity == BucketIdentity::Conflicting
        && snapshot.availability != SnapshotAvailability::Ambiguous
    {
        return Err(StorageError::LiveAccountSnapshotInvalid);
    }
    if snapshot.bucket_identity != BucketIdentity::MainCodex
        && (five_hour.availability == MeterAvailability::Available
            || weekly.availability == MeterAvailability::Available)
    {
        return Err(StorageError::LiveAccountSnapshotInvalid);
    }

    Ok(StoredLiveAccountSnapshot {
        snapshot_id: snapshot_id.to_owned(),
        storage_schema_version: LIVE_ACCOUNT_QUOTA_STORAGE_SCHEMA_VERSION,
        schema_version: SNAPSHOT_SCHEMA_VERSION,
        source: LIVE_ACCOUNT_QUOTA_SOURCE,
        acquired_at,
        availability: snapshot.availability,
        bucket_identity: snapshot.bucket_identity,
        five_hour,
        weekly,
        warnings,
        evidence_scope: LIVE_ACCOUNT_QUOTA_EVIDENCE_SCOPE,
        task_attribution: LIVE_ACCOUNT_QUOTA_TASK_ATTRIBUTION,
        forecast_qualification: LIVE_ACCOUNT_QUOTA_FORECAST_QUALIFICATION,
        provenance: SNAPSHOT_PROVENANCE,
        payload_sha256: String::new(),
    })
}

fn normalize_meter(meter: &MeterSnapshot) -> Result<StoredLiveQuotaMeter, StorageError> {
    match meter.availability {
        MeterAvailability::Available => {
            let used = meter
                .used_percent
                .filter(|value| value.is_finite() && (0.0..=100.0).contains(value))
                .ok_or(StorageError::LiveAccountSnapshotInvalid)?;
            let used = if used == 0.0 { 0.0 } else { used };
            let remaining = meter
                .remaining_percent
                .filter(|value| value.is_finite() && *value == 100.0 - used)
                .ok_or(StorageError::LiveAccountSnapshotInvalid)?;
            if meter.used_percent_provenance != "observed"
                || meter.remaining_percent_provenance != "derived"
            {
                return Err(StorageError::LiveAccountSnapshotInvalid);
            }
            let reset_at_utc = meter
                .reset_at_utc
                .as_deref()
                .map(|timestamp| normalize_utc_timestamp(timestamp).map(|(canonical, _)| canonical))
                .transpose()?;
            Ok(StoredLiveQuotaMeter {
                availability: MeterAvailability::Available,
                used_percent: Some(used),
                used_percent_provenance: "observed",
                remaining_percent_derived: Some(remaining),
                remaining_percent_provenance: "derived",
                reset_at_utc,
            })
        }
        MeterAvailability::Unavailable | MeterAvailability::Ambiguous => {
            if meter.used_percent.is_some()
                || meter.remaining_percent.is_some()
                || meter.reset_at_utc.is_some()
                || meter.used_percent_provenance != "unavailable"
                || meter.remaining_percent_provenance != "unavailable"
            {
                return Err(StorageError::LiveAccountSnapshotInvalid);
            }
            Ok(StoredLiveQuotaMeter {
                availability: meter.availability,
                used_percent: None,
                used_percent_provenance: "unavailable",
                remaining_percent_derived: None,
                remaining_percent_provenance: "unavailable",
                reset_at_utc: None,
            })
        }
    }
}

fn normalize_utc_timestamp(value: &str) -> Result<(String, i64), StorageError> {
    let timestamp = OffsetDateTime::parse(value, &Rfc3339)
        .map_err(|_| StorageError::LiveAccountSnapshotInvalid)?;
    if timestamp.offset() != UtcOffset::UTC {
        return Err(StorageError::LiveAccountSnapshotInvalid);
    }
    let unix_ns = i64::try_from(timestamp.unix_timestamp_nanos())
        .map_err(|_| StorageError::LiveAccountSnapshotInvalid)?;
    let canonical = timestamp
        .format(&Rfc3339)
        .map_err(|_| StorageError::LiveAccountSnapshotInvalid)?;
    Ok((canonical, unix_ns))
}

fn validate_snapshot_id(snapshot_id: &str) -> Result<(), StorageError> {
    let Some(random) = snapshot_id.strip_prefix(SNAPSHOT_ID_PREFIX) else {
        return Err(StorageError::LiveAccountSnapshotCursorInvalid);
    };
    if random.len() != 32
        || !random
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(StorageError::LiveAccountSnapshotCursorInvalid);
    }
    Ok(())
}

fn snapshot_checksum(snapshot: &StoredLiveAccountSnapshot) -> Result<String, StorageError> {
    let payload = CanonicalSnapshotPayload {
        snapshot_id: &snapshot.snapshot_id,
        storage_schema_version: snapshot.storage_schema_version,
        schema_version: snapshot.schema_version,
        source: snapshot.source,
        acquired_at: &snapshot.acquired_at,
        availability: snapshot.availability,
        bucket_identity: snapshot.bucket_identity,
        five_hour: &snapshot.five_hour,
        weekly: &snapshot.weekly,
        warnings: &snapshot.warnings,
        evidence_scope: snapshot.evidence_scope,
        task_attribution: snapshot.task_attribution,
        forecast_qualification: snapshot.forecast_qualification,
        provenance: snapshot.provenance,
    };
    let bytes =
        serde_json::to_vec(&payload).map_err(|_| StorageError::LiveAccountSnapshotInvalid)?;
    Ok(Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn load_snapshot_by_id(
    connection: &Connection,
    snapshot_id: &str,
) -> Result<Option<StoredLiveAccountSnapshot>, StorageError> {
    let sql = format!(
        "SELECT {SNAPSHOT_COLUMNS} FROM live_account_quota_snapshots WHERE snapshot_id = ?1"
    );
    let raw = connection
        .query_row(&sql, [snapshot_id], read_raw_snapshot)
        .optional()
        .map_err(snapshot_read_error)?;
    raw.map(decode_snapshot).transpose()
}

fn read_raw_snapshot(row: &Row<'_>) -> rusqlite::Result<RawSnapshot> {
    Ok(RawSnapshot {
        snapshot_id: row.get(0)?,
        acquired_at_utc: row.get(1)?,
        acquired_at_unix_ns: row.get(2)?,
        storage_schema_version: row.get(3)?,
        source: row.get(4)?,
        evidence_scope: row.get(5)?,
        task_attribution: row.get(6)?,
        forecast_qualification: row.get(7)?,
        overall_availability: row.get(8)?,
        main_bucket_identity: row.get(9)?,
        five_hour_availability: row.get(10)?,
        five_hour_used_percent: row.get(11)?,
        five_hour_reset_at_utc: row.get(12)?,
        weekly_availability: row.get(13)?,
        weekly_used_percent: row.get(14)?,
        weekly_reset_at_utc: row.get(15)?,
        warning_codes: row.get(16)?,
        payload_sha256: row.get(17)?,
    })
}

fn decode_snapshot(raw: RawSnapshot) -> Result<StoredLiveAccountSnapshot, StorageError> {
    if raw.storage_schema_version != i64::from(LIVE_ACCOUNT_QUOTA_STORAGE_SCHEMA_VERSION)
        || raw.source != LIVE_ACCOUNT_QUOTA_SOURCE
        || raw.evidence_scope != LIVE_ACCOUNT_QUOTA_EVIDENCE_SCOPE
        || raw.task_attribution != LIVE_ACCOUNT_QUOTA_TASK_ATTRIBUTION
        || raw.forecast_qualification != LIVE_ACCOUNT_QUOTA_FORECAST_QUALIFICATION
    {
        return Err(StorageError::LiveAccountSnapshotCorrupt);
    }
    validate_snapshot_id(&raw.snapshot_id).map_err(|_| StorageError::LiveAccountSnapshotCorrupt)?;
    let (acquired_at, acquired_at_unix_ns) = normalize_utc_timestamp(&raw.acquired_at_utc)
        .map_err(|_| StorageError::LiveAccountSnapshotCorrupt)?;
    if acquired_at != raw.acquired_at_utc || acquired_at_unix_ns != raw.acquired_at_unix_ns {
        return Err(StorageError::LiveAccountSnapshotCorrupt);
    }
    let availability = parse_snapshot_availability(&raw.overall_availability)
        .ok_or(StorageError::LiveAccountSnapshotCorrupt)?;
    let bucket_identity = parse_bucket_identity(&raw.main_bucket_identity)
        .ok_or(StorageError::LiveAccountSnapshotCorrupt)?;
    let five_hour = decode_meter(
        &raw.five_hour_availability,
        raw.five_hour_used_percent,
        raw.five_hour_reset_at_utc,
    )?;
    let weekly = decode_meter(
        &raw.weekly_availability,
        raw.weekly_used_percent,
        raw.weekly_reset_at_utc,
    )?;
    let warnings: Vec<QuotaWarning> = serde_json::from_str(&raw.warning_codes)
        .map_err(|_| StorageError::LiveAccountSnapshotCorrupt)?;
    if warnings.len() > MAX_WARNING_CODES
        || warnings.iter().copied().collect::<HashSet<_>>().len() != warnings.len()
    {
        return Err(StorageError::LiveAccountSnapshotCorrupt);
    }
    let expected_availability = match (five_hour.availability, weekly.availability) {
        (MeterAvailability::Ambiguous, _) | (_, MeterAvailability::Ambiguous) => {
            SnapshotAvailability::Ambiguous
        }
        (MeterAvailability::Available, MeterAvailability::Available) => {
            SnapshotAvailability::Available
        }
        (MeterAvailability::Unavailable, MeterAvailability::Unavailable) => {
            SnapshotAvailability::Unavailable
        }
        _ => SnapshotAvailability::Partial,
    };
    if availability != expected_availability
        || (bucket_identity != BucketIdentity::MainCodex
            && (five_hour.availability == MeterAvailability::Available
                || weekly.availability == MeterAvailability::Available))
        || (bucket_identity == BucketIdentity::Conflicting
            && availability != SnapshotAvailability::Ambiguous)
    {
        return Err(StorageError::LiveAccountSnapshotCorrupt);
    }
    let record = StoredLiveAccountSnapshot {
        snapshot_id: raw.snapshot_id,
        storage_schema_version: LIVE_ACCOUNT_QUOTA_STORAGE_SCHEMA_VERSION,
        schema_version: SNAPSHOT_SCHEMA_VERSION,
        source: LIVE_ACCOUNT_QUOTA_SOURCE,
        acquired_at,
        availability,
        bucket_identity,
        five_hour,
        weekly,
        warnings,
        evidence_scope: LIVE_ACCOUNT_QUOTA_EVIDENCE_SCOPE,
        task_attribution: LIVE_ACCOUNT_QUOTA_TASK_ATTRIBUTION,
        forecast_qualification: LIVE_ACCOUNT_QUOTA_FORECAST_QUALIFICATION,
        provenance: SNAPSHOT_PROVENANCE,
        payload_sha256: raw.payload_sha256,
    };
    if record.payload_sha256.len() != 64
        || !record
            .payload_sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || snapshot_checksum(&record).ok().as_deref() != Some(record.payload_sha256.as_str())
    {
        return Err(StorageError::LiveAccountSnapshotCorrupt);
    }
    Ok(record)
}

fn decode_meter(
    availability: &str,
    used_percent: Option<f64>,
    reset_at_utc: Option<String>,
) -> Result<StoredLiveQuotaMeter, StorageError> {
    let availability =
        parse_meter_availability(availability).ok_or(StorageError::LiveAccountSnapshotCorrupt)?;
    match availability {
        MeterAvailability::Available => {
            let used = used_percent
                .filter(|value| value.is_finite() && (0.0..=100.0).contains(value))
                .ok_or(StorageError::LiveAccountSnapshotCorrupt)?;
            let reset_at_utc = match reset_at_utc {
                Some(timestamp) => {
                    let (canonical, _) = normalize_utc_timestamp(&timestamp)
                        .map_err(|_| StorageError::LiveAccountSnapshotCorrupt)?;
                    if canonical != timestamp {
                        return Err(StorageError::LiveAccountSnapshotCorrupt);
                    }
                    Some(canonical)
                }
                None => None,
            };
            Ok(StoredLiveQuotaMeter {
                availability,
                used_percent: Some(used),
                used_percent_provenance: "observed",
                remaining_percent_derived: Some(100.0 - used),
                remaining_percent_provenance: "derived",
                reset_at_utc,
            })
        }
        MeterAvailability::Unavailable | MeterAvailability::Ambiguous => {
            if used_percent.is_some() || reset_at_utc.is_some() {
                return Err(StorageError::LiveAccountSnapshotCorrupt);
            }
            Ok(StoredLiveQuotaMeter {
                availability,
                used_percent: None,
                used_percent_provenance: "unavailable",
                remaining_percent_derived: None,
                remaining_percent_provenance: "unavailable",
                reset_at_utc: None,
            })
        }
    }
}

fn snapshot_availability_code(value: SnapshotAvailability) -> &'static str {
    match value {
        SnapshotAvailability::Available => "available",
        SnapshotAvailability::Partial => "partial",
        SnapshotAvailability::Unavailable => "unavailable",
        SnapshotAvailability::Ambiguous => "ambiguous",
    }
}

fn parse_snapshot_availability(value: &str) -> Option<SnapshotAvailability> {
    match value {
        "available" => Some(SnapshotAvailability::Available),
        "partial" => Some(SnapshotAvailability::Partial),
        "unavailable" => Some(SnapshotAvailability::Unavailable),
        "ambiguous" => Some(SnapshotAvailability::Ambiguous),
        _ => None,
    }
}

fn meter_availability_code(value: MeterAvailability) -> &'static str {
    match value {
        MeterAvailability::Available => "available",
        MeterAvailability::Unavailable => "unavailable",
        MeterAvailability::Ambiguous => "ambiguous",
    }
}

fn parse_meter_availability(value: &str) -> Option<MeterAvailability> {
    match value {
        "available" => Some(MeterAvailability::Available),
        "unavailable" => Some(MeterAvailability::Unavailable),
        "ambiguous" => Some(MeterAvailability::Ambiguous),
        _ => None,
    }
}

fn bucket_identity_code(value: BucketIdentity) -> &'static str {
    match value {
        BucketIdentity::MainCodex => "main_codex",
        BucketIdentity::OtherCodexRelated => "other_codex_related",
        BucketIdentity::OtherBucket => "other_bucket",
        BucketIdentity::Unknown => "unknown",
        BucketIdentity::Conflicting => "conflicting",
    }
}

fn parse_bucket_identity(value: &str) -> Option<BucketIdentity> {
    match value {
        "main_codex" => Some(BucketIdentity::MainCodex),
        "other_codex_related" => Some(BucketIdentity::OtherCodexRelated),
        "other_bucket" => Some(BucketIdentity::OtherBucket),
        "unknown" => Some(BucketIdentity::Unknown),
        "conflicting" => Some(BucketIdentity::Conflicting),
        _ => None,
    }
}

fn snapshot_read_error(error: rusqlite::Error) -> StorageError {
    match &error {
        rusqlite::Error::SqliteFailure(failure, _)
            if matches!(
                failure.code,
                rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
            ) =>
        {
            StorageError::sqlite(error)
        }
        _ => StorageError::LiveAccountSnapshotCorrupt,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codex_app_server::CodexAppServer;
    use crate::storage::sqlite::{open_with_migrations, SqliteReadStore};
    use crate::storage::{SqliteStore, StorageError};
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_PATH: AtomicU64 = AtomicU64::new(0);

    struct TestDatabase(std::path::PathBuf);

    impl TestDatabase {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!(
                "codex-meter-p045b-{}-{}.sqlite3",
                std::process::id(),
                NEXT_PATH.fetch_add(1, Ordering::Relaxed)
            )))
        }

        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }

    impl Drop for TestDatabase {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
            let _ = fs::remove_file(self.0.with_extension("sqlite3-wal"));
            let _ = fs::remove_file(self.0.with_extension("sqlite3-shm"));
        }
    }

    fn snapshot(mode: &str) -> AccountRateLimitsSnapshot {
        CodexAppServer::connect_test_server(mode)
            .unwrap()
            .read_rate_limits()
            .unwrap()
    }

    fn identified_snapshot(mode: &str, acquired_at: &str) -> AccountRateLimitsSnapshot {
        let mut value = snapshot(mode);
        value.acquired_at = acquired_at.to_owned();
        value
    }

    fn fixed_id(value: u8) -> String {
        format!("lqs_{:032x}", u128::from(value))
    }

    #[test]
    fn fresh_schema_separates_live_snapshots_and_keeps_provenance_fixed() {
        let database = TestDatabase::new();
        let mut store = SqliteStore::open(database.path()).unwrap();
        assert_eq!(store.info().latest_applied_migration, 7);
        let snapshot = snapshot("rate_limits");
        let saved = store.save_live_account_snapshot(&snapshot).unwrap();
        assert_eq!(saved.persistence, LiveSnapshotSaveDisposition::Persisted);
        assert!(saved.snapshot.snapshot_id.starts_with("lqs_"));
        assert_eq!(saved.snapshot.source, LIVE_ACCOUNT_QUOTA_SOURCE);
        assert_eq!(saved.snapshot.evidence_scope, "account_level");
        assert_eq!(saved.snapshot.task_attribution, "not_established");
        assert_eq!(saved.snapshot.forecast_qualification, "not_established");
        assert_eq!(saved.snapshot.five_hour.used_percent, Some(42.0));
        assert_eq!(saved.snapshot.weekly.used_percent, Some(0.0));
        assert_eq!(saved.snapshot.weekly.remaining_percent_derived, Some(100.0));
        assert_eq!(saved.snapshot.payload_sha256.len(), 64);
        let live_count: i64 = store
            .connection()
            .query_row(
                "SELECT count(*) FROM live_account_quota_snapshots",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let rollout_count: i64 = store
            .connection()
            .query_row("SELECT count(*) FROM quota_snapshot_evidence", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(live_count, 1);
        assert_eq!(rollout_count, 0);
    }

    #[test]
    fn fractional_observed_percent_survives_file_backed_round_trip() {
        let database = TestDatabase::new();
        let mut snapshot = snapshot("rate_limits");
        snapshot.five_hour.used_percent = Some(42.5);
        snapshot.five_hour.remaining_percent = Some(57.5);
        let mut store = SqliteStore::open(database.path()).unwrap();
        let saved = store.save_live_account_snapshot(&snapshot).unwrap();
        drop(store);

        let mut reader = SqliteReadStore::open(database.path()).unwrap();
        let page = reader.live_account_snapshot_history(10, None).unwrap();
        assert_eq!(page.snapshots[0].snapshot_id, saved.snapshot.snapshot_id);
        assert_eq!(page.snapshots[0].five_hour.used_percent, Some(42.5));
        assert_eq!(
            page.snapshots[0].five_hour.remaining_percent_derived,
            Some(57.5)
        );
    }

    #[test]
    fn fresh_live_history_returns_an_empty_bounded_page() {
        let database = TestDatabase::new();
        drop(SqliteStore::open(database.path()).unwrap());
        let mut store = SqliteReadStore::open(database.path()).unwrap();
        assert!(store
            .live_account_snapshot_history(20, None)
            .unwrap()
            .snapshots
            .is_empty());
    }

    #[test]
    fn repeated_user_acquisitions_remain_distinct_even_with_equal_values() {
        let mut store = SqliteStore::open_in_memory().unwrap();
        let snapshot = snapshot("rate_limits");
        let first = store.save_live_account_snapshot(&snapshot).unwrap();
        let second = store.save_live_account_snapshot(&snapshot).unwrap();
        assert_ne!(first.snapshot.snapshot_id, second.snapshot.snapshot_id);
        assert_eq!(
            first.snapshot.five_hour.used_percent,
            second.snapshot.five_hour.used_percent
        );
        let count: i64 = store
            .connection()
            .query_row(
                "SELECT count(*) FROM live_account_quota_snapshots",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn exact_retry_is_idempotent_and_conflicting_payload_fails_closed() {
        let mut store = SqliteStore::open_in_memory().unwrap();
        let id = fixed_id(1);
        let mut original = identified_snapshot("rate_limits", "2026-11-19T01:02:03.123456789Z");
        let first = store.retry_live_account_snapshot(&id, &original).unwrap();
        let retry = store.retry_live_account_snapshot(&id, &original).unwrap();
        assert_eq!(first.persistence, LiveSnapshotSaveDisposition::Persisted);
        assert_eq!(
            retry.persistence,
            LiveSnapshotSaveDisposition::IdempotentRetry
        );
        assert_eq!(first.snapshot.payload_sha256, retry.snapshot.payload_sha256);

        original.five_hour.used_percent = Some(43.0);
        original.five_hour.remaining_percent = Some(57.0);
        assert!(matches!(
            store.retry_live_account_snapshot(&id, &original),
            Err(StorageError::LiveAccountSnapshotConflict)
        ));
        let count: i64 = store
            .connection()
            .query_row(
                "SELECT count(*) FROM live_account_quota_snapshots",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn partial_unavailable_and_ambiguous_snapshots_preserve_missing_values() {
        let mut store = SqliteStore::open_in_memory().unwrap();
        let mut partial = snapshot("rate_limits");
        partial.weekly.availability = MeterAvailability::Unavailable;
        partial.weekly.used_percent = None;
        partial.weekly.used_percent_provenance = "unavailable";
        partial.weekly.remaining_percent = None;
        partial.weekly.remaining_percent_provenance = "unavailable";
        partial.weekly.reset_at_utc = None;
        partial.availability = SnapshotAvailability::Partial;
        partial.warnings.push(QuotaWarning::MissingWeekly);
        let stored_partial = store.save_live_account_snapshot(&partial).unwrap();
        assert_eq!(stored_partial.snapshot.weekly.used_percent, None);
        assert_eq!(
            stored_partial.snapshot.weekly.availability,
            MeterAvailability::Unavailable
        );

        let unavailable = snapshot("rate_limits_missing");
        let stored_unavailable = store.save_live_account_snapshot(&unavailable).unwrap();
        assert_eq!(
            stored_unavailable.snapshot.availability,
            SnapshotAvailability::Unavailable
        );
        assert_eq!(stored_unavailable.snapshot.five_hour.used_percent, None);
        assert!(!stored_unavailable.snapshot.warnings.is_empty());

        let ambiguous = snapshot("rate_limits_duplicate");
        let stored_ambiguous = store.save_live_account_snapshot(&ambiguous).unwrap();
        assert_eq!(
            stored_ambiguous.snapshot.availability,
            SnapshotAvailability::Ambiguous
        );
        assert_eq!(stored_ambiguous.snapshot.five_hour.used_percent, None);

        let mut weekly_only = snapshot("rate_limits");
        weekly_only.five_hour.availability = MeterAvailability::Unavailable;
        weekly_only.five_hour.used_percent = None;
        weekly_only.five_hour.used_percent_provenance = "unavailable";
        weekly_only.five_hour.remaining_percent = None;
        weekly_only.five_hour.remaining_percent_provenance = "unavailable";
        weekly_only.five_hour.reset_at_utc = None;
        weekly_only.availability = SnapshotAvailability::Partial;
        weekly_only.warnings.push(QuotaWarning::MissingFiveHour);
        let stored_weekly_only = store.save_live_account_snapshot(&weekly_only).unwrap();
        assert_eq!(stored_weekly_only.snapshot.five_hour.used_percent, None);
        assert_eq!(stored_weekly_only.snapshot.weekly.used_percent, Some(0.0));
    }

    #[test]
    fn observed_hundred_zero_and_missing_reset_are_preserved_without_carry_forward() {
        let mut store = SqliteStore::open_in_memory().unwrap();
        let legacy = snapshot("rate_limits_legacy");
        let stored_legacy = store.save_live_account_snapshot(&legacy).unwrap();
        assert_eq!(stored_legacy.snapshot.five_hour.used_percent, Some(100.0));
        assert_eq!(
            stored_legacy.snapshot.five_hour.remaining_percent_derived,
            Some(0.0)
        );
        assert_eq!(stored_legacy.snapshot.weekly.used_percent, None);

        let mut older = identified_snapshot("rate_limits", "2026-11-19T00:00:00.100000000Z");
        older.weekly.used_percent = Some(8.0);
        older.weekly.remaining_percent = Some(92.0);
        let old_id = fixed_id(5);
        store.retry_live_account_snapshot(&old_id, &older).unwrap();

        let mut newer = older.clone();
        newer.acquired_at = "2026-11-19T00:00:00.100000001Z".into();
        newer.five_hour.used_percent = Some(0.0);
        newer.five_hour.remaining_percent = Some(100.0);
        newer.weekly.availability = MeterAvailability::Unavailable;
        newer.weekly.used_percent = None;
        newer.weekly.used_percent_provenance = "unavailable";
        newer.weekly.remaining_percent = None;
        newer.weekly.remaining_percent_provenance = "unavailable";
        newer.weekly.reset_at_utc = None;
        newer.availability = SnapshotAvailability::Partial;
        newer.warnings = vec![QuotaWarning::MissingWeekly];
        let new_id = fixed_id(6);
        store.retry_live_account_snapshot(&new_id, &newer).unwrap();

        // Read through the same file-backed read-only API used by the CLI.
        let database = TestDatabase::new();
        let mut writer = SqliteStore::open(database.path()).unwrap();
        writer.retry_live_account_snapshot(&old_id, &older).unwrap();
        writer.retry_live_account_snapshot(&new_id, &newer).unwrap();
        drop(writer);
        let mut reader = SqliteReadStore::open(database.path()).unwrap();
        let page = reader.live_account_snapshot_history(1, None).unwrap();
        assert_eq!(page.snapshots[0].snapshot_id, new_id);
        assert_eq!(page.snapshots[0].five_hour.used_percent, Some(0.0));
        assert_eq!(page.snapshots[0].weekly.used_percent, None);
        let older_page = reader
            .live_account_snapshot_history(1, page.next_cursor.as_deref())
            .unwrap();
        assert_eq!(older_page.snapshots[0].snapshot_id, old_id);
        assert_eq!(older_page.snapshots[0].weekly.used_percent, Some(8.0));
    }

    #[test]
    fn storage_revalidates_timestamp_bucket_status_percentage_and_warning_bounds() {
        let mut store = SqliteStore::open_in_memory().unwrap();
        let mut invalid = snapshot("rate_limits");
        invalid.acquired_at = "yesterday-ish".into();
        assert!(matches!(
            store.save_live_account_snapshot(&invalid),
            Err(StorageError::LiveAccountSnapshotInvalid)
        ));

        let mut invalid = snapshot("rate_limits");
        invalid.five_hour.used_percent = Some(f64::INFINITY);
        assert!(matches!(
            store.save_live_account_snapshot(&invalid),
            Err(StorageError::LiveAccountSnapshotInvalid)
        ));

        let mut invalid = snapshot("rate_limits");
        invalid.five_hour.used_percent = Some(101.0);
        invalid.five_hour.remaining_percent = Some(-1.0);
        assert!(matches!(
            store.save_live_account_snapshot(&invalid),
            Err(StorageError::LiveAccountSnapshotInvalid)
        ));

        let mut invalid = snapshot("rate_limits");
        invalid.five_hour.reset_at_utc = Some("not-a-utc-time".into());
        assert!(matches!(
            store.save_live_account_snapshot(&invalid),
            Err(StorageError::LiveAccountSnapshotInvalid)
        ));

        let mut invalid = snapshot("rate_limits");
        invalid.schema_version = "2.0.0";
        assert!(matches!(
            store.save_live_account_snapshot(&invalid),
            Err(StorageError::LiveAccountSnapshotInvalid)
        ));

        let mut invalid = snapshot("rate_limits");
        invalid.bucket_identity = BucketIdentity::Unknown;
        assert!(matches!(
            store.save_live_account_snapshot(&invalid),
            Err(StorageError::LiveAccountSnapshotInvalid)
        ));

        let mut invalid = snapshot("rate_limits");
        invalid.warnings = vec![QuotaWarning::NoQuotaData; 33];
        assert!(matches!(
            store.save_live_account_snapshot(&invalid),
            Err(StorageError::LiveAccountSnapshotInvalid)
        ));
    }

    #[test]
    fn migration_constraints_reject_out_of_range_percent_and_mutable_provenance() {
        let mut store = SqliteStore::open_in_memory().unwrap();
        let saved = store
            .save_live_account_snapshot(&snapshot("rate_limits"))
            .unwrap();
        assert!(store
            .connection()
            .execute(
                "UPDATE live_account_quota_snapshots SET five_hour_used_percent = 100.1 WHERE snapshot_id = ?1",
                [&saved.snapshot.snapshot_id],
            )
            .is_err());
        assert!(store
            .connection()
            .execute(
                "UPDATE live_account_quota_snapshots SET source = 'provider_other' WHERE snapshot_id = ?1",
                [&saved.snapshot.snapshot_id],
            )
            .is_err());
        let source: String = store
            .connection()
            .query_row(
                "SELECT source FROM live_account_quota_snapshots WHERE snapshot_id = ?1",
                [&saved.snapshot.snapshot_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(source, LIVE_ACCOUNT_QUOTA_SOURCE);
    }

    #[test]
    fn repeated_typed_warning_codes_are_canonicalized_without_losing_diagnostics() {
        let mut snapshot = snapshot("rate_limits");
        snapshot.warnings.extend([
            QuotaWarning::UnsupportedDuration,
            QuotaWarning::UnsupportedDuration,
        ]);
        let mut store = SqliteStore::open_in_memory().unwrap();
        let saved = store.save_live_account_snapshot(&snapshot).unwrap();
        assert_eq!(
            saved
                .snapshot
                .warnings
                .iter()
                .filter(|warning| **warning == QuotaWarning::UnsupportedDuration)
                .count(),
            1
        );
    }

    #[test]
    fn timestamp_keyset_pagination_preserves_fractional_and_equal_time_order() {
        // Use a file-backed database for a separate genuinely read-only connection.
        let database = TestDatabase::new();
        let mut writer = SqliteStore::open(database.path()).unwrap();
        for (id, time) in [
            (fixed_id(1), "2026-11-19T00:00:00.100000001Z"),
            (fixed_id(3), "2026-11-19T00:00:00.100000001Z"),
            (fixed_id(2), "2026-11-19T00:00:00.100000002Z"),
            (fixed_id(4), "2026-11-19T00:00:00.100000000Z"),
        ] {
            let snapshot = identified_snapshot("rate_limits", time);
            writer.retry_live_account_snapshot(&id, &snapshot).unwrap();
        }
        let mut reader = SqliteReadStore::open(database.path()).unwrap();
        let first = reader.live_account_snapshot_history(2, None).unwrap();
        assert_eq!(
            first
                .snapshots
                .iter()
                .map(|s| s.snapshot_id.as_str())
                .collect::<Vec<_>>(),
            vec![fixed_id(2).as_str(), fixed_id(3).as_str()]
        );
        let cursor = first.next_cursor.as_deref().unwrap();
        let second = reader
            .live_account_snapshot_history(2, Some(cursor))
            .unwrap();
        assert_eq!(
            second
                .snapshots
                .iter()
                .map(|s| s.snapshot_id.as_str())
                .collect::<Vec<_>>(),
            vec![fixed_id(1).as_str(), fixed_id(4).as_str()]
        );
        assert_eq!(second.next_cursor, None);
    }

    #[test]
    fn history_rejects_foreign_or_unknown_cursor_and_does_not_mix_rollout_samples() {
        let database = TestDatabase::new();
        let mut writer = SqliteStore::open(database.path()).unwrap();
        let snapshot = snapshot("rate_limits");
        writer.save_live_account_snapshot(&snapshot).unwrap();
        writer.connection().execute(
            "INSERT INTO quota_snapshot_evidence (sample_id, source_id, session_id, observed_at, meter_type, used_percent, reset_at, plan, codex_version, evidence_status, schema_version, evidence_sha256) VALUES ('rollout-sample', 'local', NULL, '2026-11-19T00:00:00Z', 'five_hour', 9.0, NULL, NULL, NULL, 'observed', 1, ?1)",
            ["0".repeat(64)],
        ).unwrap();
        drop(writer);
        let mut reader = SqliteReadStore::open(database.path()).unwrap();
        let page = reader.live_account_snapshot_history(20, None).unwrap();
        assert_eq!(page.snapshots.len(), 1);
        assert!(!page
            .snapshots
            .iter()
            .any(|item| item.snapshot_id == "rollout-sample"));
        assert!(matches!(
            reader.live_account_snapshot_history(20, Some("rollout-sample")),
            Err(StorageError::LiveAccountSnapshotCursorInvalid)
        ));
        assert!(matches!(
            reader.live_account_snapshot_history(20, Some("lqs_000000000000000000000000000000ff")),
            Err(StorageError::LiveAccountSnapshotCursorInvalid)
        ));
    }

    #[test]
    fn failed_insert_rolls_back_and_corrupt_payload_checksum_is_rejected() {
        let database = TestDatabase::new();
        let mut store = SqliteStore::open(database.path()).unwrap();
        store.connection().execute_batch(
            "CREATE TRIGGER fail_live_insert BEFORE INSERT ON live_account_quota_snapshots BEGIN SELECT RAISE(ABORT, 'forced failure'); END;",
        ).unwrap();
        assert!(store
            .save_live_account_snapshot(&snapshot("rate_limits"))
            .is_err());
        let count: i64 = store
            .connection()
            .query_row(
                "SELECT count(*) FROM live_account_quota_snapshots",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);

        store
            .connection()
            .execute_batch("DROP TRIGGER fail_live_insert;")
            .unwrap();
        let saved = store
            .save_live_account_snapshot(&snapshot("rate_limits"))
            .unwrap();
        store.connection().execute(
            "UPDATE live_account_quota_snapshots SET five_hour_used_percent = 41 WHERE snapshot_id = ?1",
            [&saved.snapshot.snapshot_id],
        ).unwrap();
        drop(store);
        let mut reader = SqliteReadStore::open(database.path()).unwrap();
        assert!(matches!(
            reader.live_account_snapshot_history(10, None),
            Err(StorageError::LiveAccountSnapshotCorrupt)
        ));
    }

    #[test]
    fn migration_from_v6_preserves_rollout_quota_and_observation_rows_and_checksums() {
        let database = TestDatabase::new();
        {
            let store = open_with_migrations(
                database.path(),
                false,
                &super::super::migrations::MIGRATIONS[..6],
            )
            .unwrap();
            store.connection().execute(
                "INSERT INTO quota_snapshot_evidence (sample_id, source_id, session_id, observed_at, meter_type, used_percent, reset_at, plan, codex_version, evidence_status, schema_version, evidence_sha256) VALUES ('old-quota', 'source', NULL, '2026-11-19T00:00:00Z', 'five_hour', 5.0, NULL, NULL, NULL, 'observed', 1, ?1)",
                ["0".repeat(64)],
            ).unwrap();
            store.connection().execute(
                "INSERT INTO observations (observation_id, schema_version, lifecycle_state, summary_quality, token_validity, token_quality, five_hour_validity, five_hour_quality, five_hour_reset_status, weekly_validity, weekly_quality, weekly_reset_status, observation_json, observation_sha256, storage_revision) VALUES ('old-observation', '1.0.0', 'finalized', 'C', 'unavailable', 'X', 'unavailable', 'X', 'unavailable', 'unavailable', 'X', 'unavailable', '{}', ?1, 1)",
                ["0".repeat(64)],
            ).unwrap();
        }
        let v6_bytes_before = fs::read(database.path()).unwrap();
        {
            let mut reader = SqliteReadStore::open(database.path()).unwrap();
            assert_eq!(
                reader
                    .live_account_snapshot_history(20, None)
                    .unwrap()
                    .snapshots
                    .len(),
                0
            );
        }
        assert_eq!(v6_bytes_before, fs::read(database.path()).unwrap());
        let old_checksums = super::super::migrations::MIGRATIONS[..6]
            .iter()
            .map(|migration| super::super::migrations::checksum(migration.sql))
            .collect::<Vec<_>>();
        let store = SqliteStore::open(database.path()).unwrap();
        assert_eq!(store.info().latest_applied_migration, 7);
        let quotas: i64 = store
            .connection()
            .query_row(
                "SELECT count(*) FROM quota_snapshot_evidence WHERE sample_id = 'old-quota'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let observations: i64 = store
            .connection()
            .query_row(
                "SELECT count(*) FROM observations WHERE observation_id = 'old-observation'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(quotas, 1);
        assert_eq!(observations, 1);
        for (index, expected) in old_checksums.iter().enumerate() {
            let actual: String = store
                .connection()
                .query_row(
                    "SELECT checksum_sha256 FROM schema_migrations WHERE version = ?1",
                    [index as i64 + 1],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(&actual, expected);
        }
    }

    #[test]
    fn readonly_history_does_not_change_database_bytes_and_future_versions_fail_safely() {
        let database = TestDatabase::new();
        let mut writer = SqliteStore::open(database.path()).unwrap();
        writer
            .save_live_account_snapshot(&snapshot("rate_limits"))
            .unwrap();
        drop(writer);
        let before = fs::read(database.path()).unwrap();
        {
            let mut reader = SqliteReadStore::open(database.path()).unwrap();
            assert_eq!(
                reader
                    .live_account_snapshot_history(10, None)
                    .unwrap()
                    .snapshots
                    .len(),
                1
            );
        }
        let after = fs::read(database.path()).unwrap();
        assert_eq!(before, after);

        let writer = SqliteStore::open(database.path()).unwrap();
        writer.connection().execute(
            "INSERT INTO schema_migrations (version, name, checksum_sha256) VALUES (8, 'future', ?1)",
            ["0".repeat(64)],
        ).unwrap();
        drop(writer);
        assert!(matches!(
            SqliteReadStore::open(database.path()),
            Err(StorageError::DatabaseTooNew { .. })
        ));
    }

    #[test]
    fn read_only_open_rejects_a_missing_version_seven_snapshot_table() {
        let database = TestDatabase::new();
        let store = SqliteStore::open(database.path()).unwrap();
        store
            .connection()
            .execute_batch("DROP TABLE live_account_quota_snapshots")
            .unwrap();
        drop(store);
        assert!(matches!(
            SqliteReadStore::open(database.path()),
            Err(StorageError::MigrationHistoryCorrupt { .. })
        ));
    }
}
