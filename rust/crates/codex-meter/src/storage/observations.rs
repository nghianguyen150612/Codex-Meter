use std::fmt;

use rusqlite::{params, params_from_iter, OptionalExtension, Row, Transaction};
use sha2::{Digest, Sha256};
use time::{format_description::well_known::Rfc3339, OffsetDateTime, UtcOffset};

use crate::telemetry::{
    ConfigurationIdentity, ConfigurationValue, EvidenceValidity, MetricAvailability,
    MetricProvenance, NormalizedObservation, ObservationLifecycle, QualityGrade, ResetStatus,
    SchemaVersion, TokenMetric,
};

use super::sqlite::StorageError;

const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
const DEFAULT_QUERY_LIMIT: u32 = 100;
const MAX_QUERY_LIMIT: u32 = 1_000;
const OBSERVATION_COLUMNS: &str = "observation_id, schema_version, task_id, session_id, source_instance_id, lifecycle_state, started_at, ended_at, finalized_at, duration_ms, summary_quality, plan, model, reasoning_level, speed_mode, codex_version, token_validity, token_quality, raw_total, five_hour_validity, five_hour_quality, five_hour_delta, five_hour_reset_status, weekly_validity, weekly_quality, weekly_delta, weekly_reset_status, observation_json, observation_sha256, storage_revision";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredObservation {
    pub observation: NormalizedObservation,
    pub storage_revision: u64,
    pub payload_sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObservationPageCursor {
    pub order_time: String,
    pub observation_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObservationPage {
    pub observations: Vec<StoredObservation>,
    pub next_cursor: Option<ObservationPageCursor>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObservationQuery {
    pub lifecycle_state: Option<ObservationLifecycle>,
    pub plan: Option<String>,
    pub model: Option<String>,
    pub reasoning_level: Option<String>,
    pub speed_mode: Option<String>,
    pub summary_quality: Option<QualityGrade>,
    pub finalized_after: Option<String>,
    pub finalized_before: Option<String>,
    pub terminal_only: bool,
    pub provisional_only: bool,
    pub limit: u32,
    pub page_cursor: Option<ObservationPageCursor>,
}

impl Default for ObservationQuery {
    fn default() -> Self {
        Self {
            lifecycle_state: None,
            plan: None,
            model: None,
            reasoning_level: None,
            speed_mode: None,
            summary_quality: None,
            finalized_after: None,
            finalized_before: None,
            terminal_only: false,
            provisional_only: false,
            limit: DEFAULT_QUERY_LIMIT,
            page_cursor: None,
        }
    }
}

impl ObservationQuery {
    pub fn new(limit: u32) -> Self {
        Self {
            limit,
            ..Self::default()
        }
    }

    pub fn validate(&self) -> Result<(), StorageError> {
        if self.limit == 0 || self.limit > MAX_QUERY_LIMIT {
            return Err(StorageError::ObservationQueryInvalid);
        }
        for timestamp in [&self.finalized_after, &self.finalized_before]
            .into_iter()
            .flatten()
        {
            canonical_timestamp_projection(timestamp)
                .map_err(|_| StorageError::ObservationQueryInvalid)?;
        }
        if let Some(cursor) = &self.page_cursor {
            if cursor.observation_id.is_empty()
                || (!cursor.order_time.is_empty()
                    && canonical_timestamp_projection(&cursor.order_time).ok()
                        != Some(cursor.order_time.clone()))
            {
                return Err(StorageError::ObservationQueryInvalid);
            }
        }
        if let (Some(after), Some(before)) = (&self.finalized_after, &self.finalized_before) {
            if canonical_timestamp_projection(after)
                .map_err(|_| StorageError::ObservationQueryInvalid)?
                > canonical_timestamp_projection(before)
                    .map_err(|_| StorageError::ObservationQueryInvalid)?
            {
                return Err(StorageError::ObservationQueryInvalid);
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
struct ObservationProjection {
    schema_version: &'static str,
    task_id: Option<String>,
    session_id: Option<String>,
    source_instance_id: Option<String>,
    lifecycle_state: &'static str,
    started_at: Option<String>,
    ended_at: Option<String>,
    finalized_at: Option<String>,
    duration_ms: Option<i64>,
    summary_quality: &'static str,
    plan: Option<String>,
    model: Option<String>,
    reasoning_level: Option<String>,
    speed_mode: Option<String>,
    codex_version: Option<String>,
    token_validity: &'static str,
    token_quality: &'static str,
    raw_total: Option<i64>,
    five_hour_validity: &'static str,
    five_hour_quality: &'static str,
    five_hour_delta: Option<f64>,
    five_hour_reset_status: &'static str,
    weekly_validity: &'static str,
    weekly_quality: &'static str,
    weekly_delta: Option<f64>,
    weekly_reset_status: &'static str,
}

impl ObservationProjection {
    fn from_observation(observation: &NormalizedObservation) -> Result<Self, StorageError> {
        validate_observation(observation)?;
        Ok(Self {
            schema_version: "1.0.0",
            task_id: observation.task_id.clone(),
            session_id: observation.session_id.clone(),
            source_instance_id: observation.source_instance_id.clone(),
            lifecycle_state: lifecycle_name(observation.lifecycle_state),
            started_at: canonical_optional_timestamp(observation.timing.started_at.as_deref())?,
            ended_at: canonical_optional_timestamp(observation.timing.ended_at.as_deref())?,
            finalized_at: canonical_optional_timestamp(observation.timing.finalized_at.as_deref())?,
            duration_ms: observation
                .timing
                .duration_ms
                .map(|value| i64::try_from(value).map_err(|_| StorageError::ObservationCorrupt))
                .transpose()?,
            summary_quality: quality_name(observation.summary_quality),
            plan: available_config(&observation.configuration.plan),
            model: available_config(&observation.configuration.model),
            reasoning_level: available_config(&observation.configuration.reasoning_level),
            speed_mode: available_config(&observation.configuration.speed_mode),
            codex_version: available_config(&observation.configuration.codex_version),
            token_validity: validity_name(observation.token_evidence.status.validity),
            token_quality: quality_name(observation.token_evidence.status.quality),
            raw_total: available_u64(
                observation
                    .token_evidence
                    .raw_token_counters
                    .raw_total
                    .availability,
                observation
                    .token_evidence
                    .raw_token_counters
                    .raw_total
                    .value,
            )?,
            five_hour_validity: validity_name(observation.quota_evidence.five_hour.status.validity),
            five_hour_quality: quality_name(observation.quota_evidence.five_hour.status.quality),
            five_hour_delta: observation.quota_evidence.five_hour.delta_percentage_points,
            five_hour_reset_status: reset_name(observation.quota_evidence.five_hour.reset_status),
            weekly_validity: validity_name(observation.quota_evidence.weekly.status.validity),
            weekly_quality: quality_name(observation.quota_evidence.weekly.status.quality),
            weekly_delta: observation.quota_evidence.weekly.delta_percentage_points,
            weekly_reset_status: reset_name(observation.quota_evidence.weekly.reset_status),
        })
    }
}

pub(crate) fn canonical_payload(
    observation: &NormalizedObservation,
) -> Result<(String, String), StorageError> {
    let projection = ObservationProjection::from_observation(observation)?;
    let bytes = serde_json::to_vec(observation).map_err(|_| StorageError::ObservationDecode)?;
    let json = String::from_utf8(bytes).map_err(|_| StorageError::ObservationDecode)?;
    let checksum = Sha256::digest(json.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let _ = projection;
    Ok((json, checksum))
}

pub(crate) fn save_observation_in_transaction(
    transaction: &Transaction<'_>,
    observation: &NormalizedObservation,
    expected_revision: Option<u64>,
) -> Result<StoredObservation, StorageError> {
    let projection = ObservationProjection::from_observation(observation)?;
    let (json, checksum) = canonical_payload(observation)?;
    let existing = load_by_id(transaction, &observation.observation_id)?;
    let Some(existing) = existing else {
        if expected_revision.is_some() {
            return Err(StorageError::ObservationRevisionConflict {
                expected: expected_revision,
                actual: None,
            });
        }
        insert_observation(transaction, observation, &projection, &json, &checksum, 1)?;
        return Ok(StoredObservation {
            observation: observation.clone(),
            storage_revision: 1,
            payload_sha256: checksum,
        });
    };

    if existing.payload_sha256 == checksum && existing.observation == *observation {
        return Ok(existing);
    }
    enforce_identity(&existing.observation, observation)?;
    if is_terminal(existing.observation.lifecycle_state) {
        return Err(StorageError::ObservationTerminalConflict);
    }
    if lifecycle_rank(observation.lifecycle_state)
        < lifecycle_rank(existing.observation.lifecycle_state)
    {
        return Err(StorageError::ObservationLifecycleRegression);
    }
    if expected_revision != Some(existing.storage_revision) {
        return Err(StorageError::ObservationRevisionConflict {
            expected: expected_revision,
            actual: Some(existing.storage_revision),
        });
    }
    let revision = existing
        .storage_revision
        .checked_add(1)
        .ok_or(StorageError::ObservationRevisionOverflow)?;
    let revision_i64 =
        i64::try_from(revision).map_err(|_| StorageError::ObservationRevisionOverflow)?;
    let changed = transaction
        .execute(
            "UPDATE observations SET schema_version = ?1, task_id = ?2, session_id = ?3, source_instance_id = ?4, lifecycle_state = ?5, started_at = ?6, ended_at = ?7, finalized_at = ?8, duration_ms = ?9, summary_quality = ?10, plan = ?11, model = ?12, reasoning_level = ?13, speed_mode = ?14, codex_version = ?15, token_validity = ?16, token_quality = ?17, raw_total = ?18, five_hour_validity = ?19, five_hour_quality = ?20, five_hour_delta = ?21, five_hour_reset_status = ?22, weekly_validity = ?23, weekly_quality = ?24, weekly_delta = ?25, weekly_reset_status = ?26, observation_json = ?27, observation_sha256 = ?28, storage_revision = ?29 WHERE observation_id = ?30 AND storage_revision = ?31",
            params_from_iter(projection_params(&projection, &json, &checksum, revision_i64, &observation.observation_id, existing.storage_revision)?),
        )
        .map_err(StorageError::sqlite)?;
    if changed != 1 {
        return Err(StorageError::ObservationRevisionConflict {
            expected: expected_revision,
            actual: Some(existing.storage_revision),
        });
    }
    Ok(StoredObservation {
        observation: observation.clone(),
        storage_revision: revision,
        payload_sha256: checksum,
    })
}

pub(crate) fn load_observation_from_connection(
    connection: &rusqlite::Connection,
    observation_id: &str,
) -> Result<Option<StoredObservation>, StorageError> {
    load_by_id(connection, observation_id)
}

pub(crate) fn list_observations_from_connection(
    connection: &rusqlite::Connection,
    query: &ObservationQuery,
) -> Result<ObservationPage, StorageError> {
    query.validate()?;
    let mut sql = format!("SELECT {OBSERVATION_COLUMNS}, COALESCE(finalized_at, ended_at, started_at, '') AS order_time FROM observations WHERE 1 = 1");
    let mut values = Vec::new();
    if query.terminal_only {
        sql.push_str(" AND lifecycle_state IN ('finalized', 'incomplete', 'invalid')");
    }
    if query.provisional_only {
        sql.push_str(" AND lifecycle_state IN ('detected', 'active', 'task_ended', 'awaiting_meter', 'reconciling')");
    }
    if let Some(value) = query.lifecycle_state {
        sql.push_str(" AND lifecycle_state = ?");
        values.push(value_string(lifecycle_name(value)));
    }
    for (column, value) in [
        ("plan", query.plan.as_ref()),
        ("model", query.model.as_ref()),
        ("reasoning_level", query.reasoning_level.as_ref()),
        ("speed_mode", query.speed_mode.as_ref()),
    ] {
        if let Some(value) = value {
            sql.push_str(" AND ");
            sql.push_str(column);
            sql.push_str(" = ?");
            values.push(value_string(value));
        }
    }
    if let Some(value) = query.summary_quality {
        sql.push_str(" AND summary_quality = ?");
        values.push(value_string(quality_name(value)));
    }
    if let Some(value) = &query.finalized_after {
        sql.push_str(" AND finalized_at IS NOT NULL AND finalized_at > ?");
        values.push(value_string(
            &canonical_timestamp_projection(value)
                .map_err(|_| StorageError::ObservationQueryInvalid)?,
        ));
    }
    if let Some(value) = &query.finalized_before {
        sql.push_str(" AND finalized_at IS NOT NULL AND finalized_at < ?");
        values.push(value_string(
            &canonical_timestamp_projection(value)
                .map_err(|_| StorageError::ObservationQueryInvalid)?,
        ));
    }
    if let Some(cursor) = &query.page_cursor {
        sql.push_str(" AND (COALESCE(finalized_at, ended_at, started_at, '') < ? OR (COALESCE(finalized_at, ended_at, started_at, '') = ? AND observation_id < ?))");
        values.push(value_string(&cursor.order_time));
        values.push(value_string(&cursor.order_time));
        values.push(value_string(&cursor.observation_id));
    }
    sql.push_str(" ORDER BY COALESCE(finalized_at, ended_at, started_at, '') DESC, observation_id DESC LIMIT ?");
    values.push(value_i64(i64::from(query.limit) + 1));

    let mut statement = connection.prepare(&sql).map_err(StorageError::sqlite)?;
    let rows = statement
        .query_map(params_from_iter(values), |row| {
            let stored = stored_from_row(row)?;
            let order_time: String = row.get(30)?;
            Ok((stored, order_time))
        })
        .map_err(StorageError::sqlite)?;
    let mut rows = rows.collect::<Result<Vec<_>, _>>().map_err(map_row_error)?;
    let has_more = rows.len() > query.limit as usize;
    if has_more {
        rows.pop();
    }
    let next_cursor = has_more
        .then(|| {
            rows.last()
                .map(|(stored, order_time)| ObservationPageCursor {
                    order_time: order_time.clone(),
                    observation_id: stored.observation.observation_id.clone(),
                })
        })
        .flatten();
    Ok(ObservationPage {
        observations: rows.into_iter().map(|(stored, _)| stored).collect(),
        next_cursor,
    })
}

fn insert_observation(
    transaction: &Transaction<'_>,
    observation: &NormalizedObservation,
    projection: &ObservationProjection,
    json: &str,
    checksum: &str,
    revision: u64,
) -> Result<(), StorageError> {
    let revision =
        i64::try_from(revision).map_err(|_| StorageError::ObservationRevisionOverflow)?;
    transaction
        .execute(
            "INSERT INTO observations (observation_id, schema_version, task_id, session_id, source_instance_id, lifecycle_state, started_at, ended_at, finalized_at, duration_ms, summary_quality, plan, model, reasoning_level, speed_mode, codex_version, token_validity, token_quality, raw_total, five_hour_validity, five_hour_quality, five_hour_delta, five_hour_reset_status, weekly_validity, weekly_quality, weekly_delta, weekly_reset_status, observation_json, observation_sha256, storage_revision) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30)",
            params_from_iter(projection_params_with_id(projection, json, checksum, revision, &observation.observation_id)),
        )
        .map_err(StorageError::sqlite)?;
    Ok(())
}

fn projection_params<'a>(
    projection: &'a ObservationProjection,
    json: &'a str,
    checksum: &'a str,
    revision: i64,
    observation_id: &'a str,
    expected_revision: u64,
) -> Result<Vec<rusqlite::types::Value>, StorageError> {
    let mut values = projection_values(projection, json, checksum, revision);
    values.push(value_string(observation_id));
    values.push(value_i64(
        i64::try_from(expected_revision).map_err(|_| StorageError::ObservationRevisionOverflow)?,
    ));
    Ok(values)
}

fn projection_params_with_id(
    projection: &ObservationProjection,
    json: &str,
    checksum: &str,
    revision: i64,
    observation_id: &str,
) -> Vec<rusqlite::types::Value> {
    let mut values = vec![value_string(observation_id)];
    values.extend(projection_values(projection, json, checksum, revision));
    values
}

fn projection_values(
    projection: &ObservationProjection,
    json: &str,
    checksum: &str,
    revision: i64,
) -> Vec<rusqlite::types::Value> {
    vec![
        value_string(projection.schema_version),
        value_opt_string(&projection.task_id),
        value_opt_string(&projection.session_id),
        value_opt_string(&projection.source_instance_id),
        value_string(projection.lifecycle_state),
        value_opt_string(&projection.started_at),
        value_opt_string(&projection.ended_at),
        value_opt_string(&projection.finalized_at),
        value_opt_i64(projection.duration_ms),
        value_string(projection.summary_quality),
        value_opt_string(&projection.plan),
        value_opt_string(&projection.model),
        value_opt_string(&projection.reasoning_level),
        value_opt_string(&projection.speed_mode),
        value_opt_string(&projection.codex_version),
        value_string(projection.token_validity),
        value_string(projection.token_quality),
        value_opt_i64(projection.raw_total),
        value_string(projection.five_hour_validity),
        value_string(projection.five_hour_quality),
        value_opt_f64(projection.five_hour_delta),
        value_string(projection.five_hour_reset_status),
        value_string(projection.weekly_validity),
        value_string(projection.weekly_quality),
        value_opt_f64(projection.weekly_delta),
        value_string(projection.weekly_reset_status),
        value_string(json),
        value_string(checksum),
        value_i64(revision),
    ]
}

fn load_by_id<T: ObservationConnection>(
    connection: &T,
    observation_id: &str,
) -> Result<Option<StoredObservation>, StorageError> {
    connection
        .query_observation(observation_id)
        .map_err(map_row_error)
}

fn stored_from_row(row: &Row<'_>) -> rusqlite::Result<StoredObservation> {
    let observation_id: String = row.get(0)?;
    let json: String = row.get(27)?;
    let checksum: String = row.get(28)?;
    let revision: i64 = row.get(29)?;
    let calculated = Sha256::digest(json.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    if calculated != checksum {
        return Err(row_error(ObservationRowError::Corrupt));
    }
    let raw: serde_json::Value =
        serde_json::from_str(&json).map_err(|_| row_error(ObservationRowError::Decode))?;
    match raw
        .get("schema_version")
        .and_then(serde_json::Value::as_str)
    {
        Some("1.0.0") => {}
        Some(_) => return Err(row_error(ObservationRowError::FormatUnsupported)),
        None => return Err(row_error(ObservationRowError::Decode)),
    }
    let observation: NormalizedObservation =
        serde_json::from_str(&json).map_err(|_| row_error(ObservationRowError::Decode))?;
    if observation.observation_id != observation_id {
        return Err(row_error(ObservationRowError::Corrupt));
    }
    validate_observation(&observation).map_err(|_| row_error(ObservationRowError::Corrupt))?;
    let projection = ObservationProjection::from_observation(&observation)
        .map_err(|_| row_error(ObservationRowError::Corrupt))?;
    verify_projection(row, &projection)?;
    let revision = u64::try_from(revision).map_err(|_| row_error(ObservationRowError::Corrupt))?;
    if revision == 0 {
        return Err(row_error(ObservationRowError::Corrupt));
    }
    Ok(StoredObservation {
        observation,
        storage_revision: revision,
        payload_sha256: checksum,
    })
}

#[derive(Debug)]
enum ObservationRowError {
    Corrupt,
    Decode,
    ProjectionMismatch,
    FormatUnsupported,
}

impl fmt::Display for ObservationRowError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Corrupt => "observation corrupt",
            Self::Decode => "observation decode",
            Self::ProjectionMismatch => "observation projection mismatch",
            Self::FormatUnsupported => "observation format unsupported",
        })
    }
}
impl std::error::Error for ObservationRowError {}

fn row_error(error: ObservationRowError) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
}

fn map_row_error(error: rusqlite::Error) -> StorageError {
    if let rusqlite::Error::FromSqlConversionFailure(_, _, source) = &error {
        if let Some(source) = source.downcast_ref::<ObservationRowError>() {
            return match source {
                ObservationRowError::Corrupt => StorageError::ObservationCorrupt,
                ObservationRowError::Decode => StorageError::ObservationDecode,
                ObservationRowError::ProjectionMismatch => {
                    StorageError::ObservationProjectionMismatch
                }
                ObservationRowError::FormatUnsupported => {
                    StorageError::ObservationFormatUnsupported
                }
            };
        }
    }
    StorageError::sqlite(error)
}

fn verify_projection(row: &Row<'_>, projection: &ObservationProjection) -> rusqlite::Result<()> {
    let values = projection_values(projection, "", "", 1);
    let indexes = [
        1usize, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24,
        25, 26,
    ];
    for (value, index) in values.into_iter().take(26).zip(indexes) {
        let actual: rusqlite::types::Value = row.get(index)?;
        if value != actual {
            return Err(row_error(ObservationRowError::ProjectionMismatch));
        }
    }
    Ok(())
}

fn validate_observation(observation: &NormalizedObservation) -> Result<(), StorageError> {
    if observation.schema_version != SchemaVersion::V1 || observation.observation_id.is_empty() {
        return Err(StorageError::ObservationCorrupt);
    }
    for value in [
        &observation.task_id,
        &observation.session_id,
        &observation.source_instance_id,
    ]
    .into_iter()
    .flatten()
    {
        if value.is_empty() {
            return Err(StorageError::ObservationCorrupt);
        }
    }
    validate_timing(observation)?;
    validate_configuration(&observation.configuration)?;
    for metric in [
        &observation.token_evidence.raw_token_counters.uncached_input,
        &observation.token_evidence.raw_token_counters.cached_input,
        &observation.token_evidence.raw_token_counters.output,
        &observation
            .token_evidence
            .raw_token_counters
            .reasoning_output,
        &observation.token_evidence.raw_token_counters.raw_total,
    ] {
        validate_metric(metric)?;
    }
    validate_status(
        observation.token_evidence.status.validity,
        observation.token_evidence.status.quality,
    )?;
    validate_quota(&observation.quota_evidence.five_hour, "five_hour")?;
    validate_quota(&observation.quota_evidence.weekly, "weekly")?;
    let expected_quality = observation
        .token_evidence
        .status
        .quality
        .max(observation.quota_evidence.five_hour.status.quality)
        .max(observation.quota_evidence.weekly.status.quality);
    if observation.summary_quality != expected_quality {
        return Err(StorageError::ObservationCorrupt);
    }
    Ok(())
}

fn validate_timing(observation: &NormalizedObservation) -> Result<(), StorageError> {
    let started = observation
        .timing
        .started_at
        .as_deref()
        .map(parse_timestamp)
        .transpose()
        .map_err(|_| StorageError::ObservationCorrupt)?;
    let ended = observation
        .timing
        .ended_at
        .as_deref()
        .map(parse_timestamp)
        .transpose()
        .map_err(|_| StorageError::ObservationCorrupt)?;
    let finalized = observation
        .timing
        .finalized_at
        .as_deref()
        .map(parse_timestamp)
        .transpose()
        .map_err(|_| StorageError::ObservationCorrupt)?;
    if let (Some(started), Some(ended)) = (started, ended) {
        if ended < started {
            return Err(StorageError::ObservationCorrupt);
        }
    }
    if let (Some(ended), Some(finalized)) = (ended, finalized) {
        if finalized < ended {
            return Err(StorageError::ObservationCorrupt);
        }
    }
    if observation
        .timing
        .duration_ms
        .is_some_and(|value| value > MAX_SAFE_INTEGER)
    {
        return Err(StorageError::ObservationCorrupt);
    }
    if !is_terminal(observation.lifecycle_state) && observation.timing.finalized_at.is_some() {
        return Err(StorageError::ObservationCorrupt);
    }
    match (started, ended, observation.timing.duration_ms) {
        (Some(started), Some(ended), Some(duration_ms)) => {
            let milliseconds = (ended - started).whole_milliseconds();
            if milliseconds < 0 || u64::try_from(milliseconds).ok() != Some(duration_ms) {
                return Err(StorageError::ObservationCorrupt);
            }
        }
        (Some(_), Some(_), None) | (None, _, Some(_)) | (_, None, Some(_)) => {
            return Err(StorageError::ObservationCorrupt);
        }
        _ => {}
    }
    Ok(())
}

fn validate_configuration(configuration: &ConfigurationIdentity) -> Result<(), StorageError> {
    for value in [
        &configuration.plan,
        &configuration.model,
        &configuration.reasoning_level,
        &configuration.speed_mode,
        &configuration.codex_version,
    ] {
        if value.availability == MetricAvailability::Available {
            if value.value.as_deref().is_none_or(str::is_empty)
                || value.provenance == MetricProvenance::Unavailable
            {
                return Err(StorageError::ObservationCorrupt);
            }
        } else if value.value.is_some()
            || value.provenance != crate::telemetry::MetricProvenance::Unavailable
        {
            return Err(StorageError::ObservationCorrupt);
        }
    }
    Ok(())
}

fn validate_metric(metric: &TokenMetric) -> Result<(), StorageError> {
    match (metric.availability, metric.value, metric.provenance) {
        (MetricAvailability::Available, Some(value), provenance)
            if value <= MAX_SAFE_INTEGER && provenance != MetricProvenance::Unavailable =>
        {
            Ok(())
        }
        (MetricAvailability::Unavailable, None, MetricProvenance::Unavailable) => Ok(()),
        _ => Err(StorageError::ObservationCorrupt),
    }
}

fn validate_status(validity: EvidenceValidity, quality: QualityGrade) -> Result<(), StorageError> {
    if validity == EvidenceValidity::Invalid && quality != QualityGrade::X {
        return Err(StorageError::ObservationCorrupt);
    }
    if validity != EvidenceValidity::Valid && quality < QualityGrade::D {
        return Err(StorageError::ObservationCorrupt);
    }
    if validity == EvidenceValidity::Valid && quality == QualityGrade::X {
        return Err(StorageError::ObservationCorrupt);
    }
    Ok(())
}

fn validate_quota(
    quota: &crate::telemetry::ObservationQuotaEvidence,
    expected_meter: &str,
) -> Result<(), StorageError> {
    let actual_meter = match quota.meter_type {
        crate::telemetry::QuotaMeterType::FiveHour => "five_hour",
        crate::telemetry::QuotaMeterType::Weekly => "weekly",
    };
    if actual_meter != expected_meter {
        return Err(StorageError::ObservationCorrupt);
    }
    validate_status(quota.status.validity, quota.status.quality)?;
    if quota.reset_status == ResetStatus::Detected && quota.delta_percentage_points.is_some() {
        return Err(StorageError::ObservationCorrupt);
    }
    if quota.status.validity == EvidenceValidity::Valid && quota.delta_percentage_points.is_none() {
        return Err(StorageError::ObservationCorrupt);
    }
    if quota.status.validity != EvidenceValidity::Valid && quota.delta_percentage_points.is_some() {
        return Err(StorageError::ObservationCorrupt);
    }
    if quota
        .delta_percentage_points
        .is_some_and(|value| !value.is_finite() || !(0.0..=100.0).contains(&value))
    {
        return Err(StorageError::ObservationCorrupt);
    }
    if quota.reset_status == ResetStatus::Detected
        && (quota.status.validity != EvidenceValidity::Invalid
            || quota.status.quality != QualityGrade::X)
    {
        return Err(StorageError::ObservationCorrupt);
    }
    Ok(())
}

fn enforce_identity(
    existing: &NormalizedObservation,
    incoming: &NormalizedObservation,
) -> Result<(), StorageError> {
    if existing.task_id != incoming.task_id
        || existing.session_id != incoming.session_id
        || existing.source_instance_id != incoming.source_instance_id
    {
        return Err(StorageError::ObservationIdentityConflict);
    }
    Ok(())
}

fn is_terminal(lifecycle: ObservationLifecycle) -> bool {
    matches!(
        lifecycle,
        ObservationLifecycle::Finalized
            | ObservationLifecycle::Incomplete
            | ObservationLifecycle::Invalid
    )
}
fn lifecycle_rank(lifecycle: ObservationLifecycle) -> u8 {
    match lifecycle {
        ObservationLifecycle::Detected => 0,
        ObservationLifecycle::Active => 1,
        ObservationLifecycle::TaskEnded => 2,
        ObservationLifecycle::AwaitingMeter => 3,
        ObservationLifecycle::Reconciling => 4,
        ObservationLifecycle::Finalized
        | ObservationLifecycle::Incomplete
        | ObservationLifecycle::Invalid => 5,
    }
}
fn lifecycle_name(value: ObservationLifecycle) -> &'static str {
    match value {
        ObservationLifecycle::Detected => "detected",
        ObservationLifecycle::Active => "active",
        ObservationLifecycle::TaskEnded => "task_ended",
        ObservationLifecycle::AwaitingMeter => "awaiting_meter",
        ObservationLifecycle::Reconciling => "reconciling",
        ObservationLifecycle::Finalized => "finalized",
        ObservationLifecycle::Incomplete => "incomplete",
        ObservationLifecycle::Invalid => "invalid",
    }
}
fn quality_name(value: QualityGrade) -> &'static str {
    match value {
        QualityGrade::A => "A",
        QualityGrade::B => "B",
        QualityGrade::C => "C",
        QualityGrade::D => "D",
        QualityGrade::X => "X",
    }
}
fn validity_name(value: EvidenceValidity) -> &'static str {
    match value {
        EvidenceValidity::Valid => "valid",
        EvidenceValidity::Incomplete => "incomplete",
        EvidenceValidity::Invalid => "invalid",
        EvidenceValidity::Unavailable => "unavailable",
    }
}
fn reset_name(value: ResetStatus) -> &'static str {
    match value {
        ResetStatus::NotDetected => "not_detected",
        ResetStatus::Detected => "detected",
        ResetStatus::Unavailable => "unavailable",
    }
}
fn available_config(value: &ConfigurationValue) -> Option<String> {
    (value.availability == MetricAvailability::Available)
        .then(|| value.value.clone())
        .flatten()
}
fn available_u64(
    availability: MetricAvailability,
    value: Option<u64>,
) -> Result<Option<i64>, StorageError> {
    if availability == MetricAvailability::Available {
        value
            .map(|value| i64::try_from(value).map_err(|_| StorageError::ObservationCorrupt))
            .transpose()
    } else {
        Ok(None)
    }
}
fn parse_timestamp(value: &str) -> Result<OffsetDateTime, ()> {
    if !value.ends_with('Z') {
        return Err(());
    }
    let timestamp = OffsetDateTime::parse(value, &Rfc3339).map_err(|_| ())?;
    if timestamp.offset() != UtcOffset::UTC {
        return Err(());
    }
    if !(0..=9999).contains(&timestamp.year()) {
        return Err(());
    }
    Ok(timestamp)
}

fn canonical_optional_timestamp(value: Option<&str>) -> Result<Option<String>, StorageError> {
    value.map(canonical_timestamp_projection).transpose()
}

pub(crate) fn canonical_timestamp_projection(value: &str) -> Result<String, StorageError> {
    let timestamp = parse_timestamp(value).map_err(|_| StorageError::ObservationCorrupt)?;
    Ok(format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:09}Z",
        timestamp.year(),
        timestamp.month() as u8,
        timestamp.day(),
        timestamp.hour(),
        timestamp.minute(),
        timestamp.second(),
        timestamp.nanosecond(),
    ))
}
fn value_string(value: impl Into<String>) -> rusqlite::types::Value {
    rusqlite::types::Value::Text(value.into())
}
fn value_opt_string(value: &Option<String>) -> rusqlite::types::Value {
    value
        .clone()
        .map_or(rusqlite::types::Value::Null, rusqlite::types::Value::Text)
}
fn value_i64(value: i64) -> rusqlite::types::Value {
    rusqlite::types::Value::Integer(value)
}
fn value_opt_i64(value: Option<i64>) -> rusqlite::types::Value {
    value.map_or(
        rusqlite::types::Value::Null,
        rusqlite::types::Value::Integer,
    )
}
fn value_opt_f64(value: Option<f64>) -> rusqlite::types::Value {
    value.map_or(rusqlite::types::Value::Null, rusqlite::types::Value::Real)
}

trait ObservationConnection {
    fn query_observation(
        &self,
        observation_id: &str,
    ) -> rusqlite::Result<Option<StoredObservation>>;
}

impl ObservationConnection for rusqlite::Connection {
    fn query_observation(
        &self,
        observation_id: &str,
    ) -> rusqlite::Result<Option<StoredObservation>> {
        self.query_row(
            &format!("SELECT {OBSERVATION_COLUMNS} FROM observations WHERE observation_id = ?1"),
            params![observation_id],
            stored_from_row,
        )
        .optional()
    }
}

impl<'a> ObservationConnection for Transaction<'a> {
    fn query_observation(
        &self,
        observation_id: &str,
    ) -> rusqlite::Result<Option<StoredObservation>> {
        self.query_row(
            &format!("SELECT {OBSERVATION_COLUMNS} FROM observations WHERE observation_id = ?1"),
            params![observation_id],
            stored_from_row,
        )
        .optional()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{ObservationQuery, SqliteStore, StorageError};

    fn fixture(name: &str) -> NormalizedObservation {
        serde_json::from_str(include_str!(concat!(
            "../../../../../fixtures/contracts/v1/",
            "observation-codex-finalized.json"
        )))
        .unwrap_or_else(|_| panic!("fixture {name} should decode"))
    }

    #[test]
    fn fixture_round_trip_and_idempotent_replay() {
        let observation = fixture("observation-codex-finalized.json");
        let (_, checksum) = canonical_payload(&observation).unwrap();
        assert_eq!(
            checksum,
            "0669eb299eb21a5df7f89acd57433872a367cb7933d4bd85ee04443098949110"
        );
        let mut store = SqliteStore::open_in_memory().unwrap();
        let first = store.save_observation(&observation, None).unwrap();
        let second = store.save_observation(&observation, Some(999)).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.storage_revision, 1);
        assert_eq!(
            store.load_observation(&observation.observation_id).unwrap(),
            Some(first)
        );
        assert_eq!(
            store
                .list_terminal_history(ObservationQuery::new(10))
                .unwrap()
                .observations
                .len(),
            1
        );
    }

    #[test]
    fn projection_and_checksum_corruption_fail_closed() {
        let observation = fixture("observation-codex-finalized.json");
        let mut store = SqliteStore::open_in_memory().unwrap();
        store.save_observation(&observation, None).unwrap();
        store
            .connection()
            .execute("UPDATE observations SET model = 'wrong'", [])
            .unwrap();
        assert!(matches!(
            store.load_observation(&observation.observation_id),
            Err(StorageError::ObservationProjectionMismatch)
        ));
        store.connection().execute("UPDATE observations SET model = NULL, observation_sha256 = '0000000000000000000000000000000000000000000000000000000000000000'", []).unwrap();
        assert!(matches!(
            store.load_observation(&observation.observation_id),
            Err(StorageError::ObservationCorrupt)
        ));
    }

    #[test]
    fn provisional_progression_and_terminal_immutability_are_enforced() {
        let terminal = fixture("observation-codex-finalized.json");
        let mut provisional = terminal.clone();
        provisional.lifecycle_state = ObservationLifecycle::AwaitingMeter;
        provisional.timing.finalized_at = None;
        let first = {
            let mut store = SqliteStore::open_in_memory().unwrap();
            let first = store.save_observation(&provisional, None).unwrap();
            let mut reconciling = provisional.clone();
            reconciling.lifecycle_state = ObservationLifecycle::Reconciling;
            let second = store
                .save_observation(&reconciling, Some(first.storage_revision))
                .unwrap();
            let mut regressed = reconciling.clone();
            regressed.lifecycle_state = ObservationLifecycle::AwaitingMeter;
            assert!(matches!(
                store.save_observation(&regressed, Some(second.storage_revision)),
                Err(StorageError::ObservationLifecycleRegression)
            ));
            let third = store
                .save_observation(&terminal, Some(second.storage_revision))
                .unwrap();
            let mut changed = terminal.clone();
            changed.token_evidence.raw_token_counters.raw_total.value = Some(999);
            assert!(matches!(
                store.save_observation(&changed, Some(third.storage_revision)),
                Err(StorageError::ObservationTerminalConflict)
            ));
            assert_eq!(third.storage_revision, 3);
            third
        };
        assert_eq!(first.storage_revision, 3);
    }

    #[test]
    fn history_filters_and_keyset_pagination_are_deterministic() {
        let template = fixture("observation-codex-finalized.json");
        let mut store = SqliteStore::open_in_memory().unwrap();
        for (id, timestamp, model) in [
            ("obs-a", "2026-10-01T02:20:00Z", "gpt-a"),
            ("obs-b", "2026-10-01T03:20:00Z", "gpt-b"),
            ("obs-c", "2026-10-01T04:20:00Z", "gpt-b"),
        ] {
            let mut observation = template.clone();
            observation.observation_id = id.to_owned();
            observation.timing.finalized_at = Some(timestamp.to_owned());
            observation.configuration.model.value = Some(model.to_owned());
            store.save_observation(&observation, None).unwrap();
        }
        let mut query = ObservationQuery::new(1);
        query.model = Some("gpt-b".to_owned());
        let first = store.list_terminal_history(query.clone()).unwrap();
        assert_eq!(
            first
                .observations
                .iter()
                .map(|row| row.observation.observation_id.as_str())
                .collect::<Vec<_>>(),
            ["obs-c"]
        );
        let cursor = first.next_cursor.clone().unwrap();
        query.page_cursor = Some(cursor);
        let second = store.list_terminal_history(query).unwrap();
        assert!(second.next_cursor.is_none());
        assert_eq!(
            second
                .observations
                .iter()
                .map(|row| row.observation.observation_id.as_str())
                .collect::<Vec<_>>(),
            ["obs-b"]
        );
        let mut exact = ObservationQuery::new(10);
        exact.model = Some("gpt-a".to_owned());
        assert_eq!(
            store
                .list_terminal_history(exact)
                .unwrap()
                .observations
                .len(),
            1
        );
    }

    #[test]
    fn observation_and_checkpoint_commit_together() {
        let observation = fixture("observation-codex-finalized.json");
        let source = crate::telemetry::SourceIdentity::new("rollout", "generation").unwrap();
        let cursor = crate::telemetry::RolloutCursor::at_start(&source);
        let mut store = SqliteStore::open_in_memory().unwrap();
        let (stored, checkpoint) = store
            .commit_observation_and_checkpoint(
                &observation,
                None,
                &source,
                &cursor,
                &crate::storage::RuntimeRecoveryState::default(),
                None,
            )
            .unwrap();
        assert_eq!(stored.storage_revision, 1);
        assert_eq!(checkpoint.revision, 1);
        assert!(store
            .load_observation(&observation.observation_id)
            .unwrap()
            .is_some());
        assert_eq!(
            store
                .load_runtime_checkpoint(&source)
                .unwrap()
                .unwrap()
                .revision,
            1
        );
    }

    #[test]
    fn composite_conflict_rolls_back_both_sides() {
        let observation = fixture("observation-codex-finalized.json");
        let source = crate::telemetry::SourceIdentity::new("rollout", "generation").unwrap();
        let cursor = crate::telemetry::RolloutCursor::at_start(&source);
        let mut store = SqliteStore::open_in_memory().unwrap();
        store
            .save_runtime_checkpoint(
                &source,
                &cursor,
                &crate::storage::RuntimeRecoveryState::default(),
                None,
            )
            .unwrap();
        let error = store
            .commit_observation_and_checkpoint(
                &observation,
                None,
                &source,
                &crate::telemetry::RolloutCursor::from_checkpoint(&source, 1, None),
                &crate::storage::RuntimeRecoveryState::default(),
                Some(999),
            )
            .unwrap_err();
        assert!(matches!(
            error,
            StorageError::CheckpointRevisionConflict { .. }
        ));
        assert!(store
            .load_observation(&observation.observation_id)
            .unwrap()
            .is_none());
        assert_eq!(
            store
                .load_runtime_checkpoint(&source)
                .unwrap()
                .unwrap()
                .revision,
            1
        );
    }

    #[test]
    fn reset_and_zero_delta_projections_preserve_null_vs_zero() {
        let mut zero = fixture("observation-codex-finalized.json");
        zero.observation_id = "obs-zero".to_owned();
        zero.quota_evidence.five_hour.delta_percentage_points = Some(0.0);
        let reset: NormalizedObservation = serde_json::from_str(include_str!(
            "../../../../../fixtures/contracts/v1/observation-codex-reset.json"
        ))
        .unwrap();
        let mut store = SqliteStore::open_in_memory().unwrap();
        store.save_observation(&zero, None).unwrap();
        store.save_observation(&reset, None).unwrap();
        let zero_delta: Option<f64> = store
            .connection()
            .query_row(
                "SELECT five_hour_delta FROM observations WHERE observation_id = 'obs-zero'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let reset_delta: Option<f64> = store
            .connection()
            .query_row(
                "SELECT five_hour_delta FROM observations WHERE observation_id = ?1",
                [&reset.observation_id],
                |row| row.get(0),
            )
            .unwrap();
        let reset_status: String = store
            .connection()
            .query_row(
                "SELECT five_hour_reset_status FROM observations WHERE observation_id = ?1",
                [&reset.observation_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(zero_delta, Some(0.0));
        assert_eq!(reset_delta, None);
        assert_eq!(reset_status, "detected");
        assert_eq!(
            store
                .load_observation(&reset.observation_id)
                .unwrap()
                .unwrap()
                .observation,
            reset
        );
    }

    #[test]
    fn malformed_unknown_and_future_payloads_fail_closed() {
        let observation = fixture("observation-codex-finalized.json");
        let mut store = SqliteStore::open_in_memory().unwrap();
        store.save_observation(&observation, None).unwrap();
        let json: String = store
            .connection()
            .query_row("SELECT observation_json FROM observations", [], |row| {
                row.get(0)
            })
            .unwrap();
        let mut value: serde_json::Value = serde_json::from_str(&json).unwrap();
        value["unknown"] = serde_json::Value::Bool(true);
        let unknown = serde_json::to_string(&value).unwrap();
        let checksum = Sha256::digest(unknown.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        store
            .connection()
            .execute(
                "UPDATE observations SET observation_json = ?1, observation_sha256 = ?2",
                (&unknown, &checksum),
            )
            .unwrap();
        assert!(matches!(
            store.load_observation(&observation.observation_id),
            Err(StorageError::ObservationDecode)
        ));
        value.as_object_mut().unwrap().remove("unknown");
        value["schema_version"] = serde_json::Value::String("2.0.0".to_owned());
        let future = serde_json::to_string(&value).unwrap();
        let checksum = Sha256::digest(future.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        store
            .connection()
            .execute(
                "UPDATE observations SET observation_json = ?1, observation_sha256 = ?2",
                (&future, &checksum),
            )
            .unwrap();
        assert!(matches!(
            store.load_observation(&observation.observation_id),
            Err(StorageError::ObservationFormatUnsupported)
        ));
    }

    #[test]
    fn canonical_timestamp_projection_normalizes_precision_and_rejects_offsets() {
        assert_eq!(
            canonical_timestamp_projection("2026-10-04T10:00:00Z").unwrap(),
            "2026-10-04T10:00:00.000000000Z"
        );
        assert_eq!(
            canonical_timestamp_projection("2026-10-04T10:00:00.123456Z").unwrap(),
            "2026-10-04T10:00:00.123456000Z"
        );
        assert!(canonical_timestamp_projection("2026-10-04T10:00:00+07:00").is_err());
    }

    #[test]
    fn mixed_precision_history_filters_and_pagination_are_chronological() {
        let template = fixture("observation-codex-finalized.json");
        let mut store = SqliteStore::open_in_memory().unwrap();
        for (id, finalized_at) in [
            ("obs-a", "2026-10-04T10:00:00Z"),
            ("obs-b", "2026-10-04T10:00:00.01Z"),
            ("obs-c", "2026-10-04T10:00:00.1Z"),
        ] {
            let mut observation = template.clone();
            observation.observation_id = id.to_owned();
            observation.timing.finalized_at = Some(finalized_at.to_owned());
            store.save_observation(&observation, None).unwrap();
        }
        let mut query = ObservationQuery::new(1);
        let mut page = store.list_terminal_history(query.clone()).unwrap();
        assert_eq!(page.observations[0].observation.observation_id, "obs-c");
        query.page_cursor = page.next_cursor.take();
        page = store.list_terminal_history(query.clone()).unwrap();
        assert_eq!(page.observations[0].observation.observation_id, "obs-b");
        query.page_cursor = page.next_cursor.take();
        page = store.list_terminal_history(query).unwrap();
        assert_eq!(page.observations[0].observation.observation_id, "obs-a");

        let mut after = ObservationQuery::new(10);
        after.finalized_after = Some("2026-10-04T10:00:00Z".to_owned());
        assert_eq!(
            store
                .list_terminal_history(after)
                .unwrap()
                .observations
                .iter()
                .map(|row| row.observation.observation_id.as_str())
                .collect::<Vec<_>>(),
            ["obs-c", "obs-b"]
        );
        let mut before = ObservationQuery::new(10);
        before.finalized_before = Some("2026-10-04T10:00:00.1Z".to_owned());
        assert_eq!(
            store
                .list_terminal_history(before)
                .unwrap()
                .observations
                .iter()
                .map(|row| row.observation.observation_id.as_str())
                .collect::<Vec<_>>(),
            ["obs-b", "obs-a"]
        );
    }

    #[test]
    fn terminal_without_finalized_at_is_valid_but_provisional_finalized_at_is_not() {
        let terminal = fixture("observation-codex-finalized.json");
        let mut store = SqliteStore::open_in_memory().unwrap();
        let mut without_finalized = terminal.clone();
        without_finalized.observation_id = "obs-terminal-no-finalized".to_owned();
        without_finalized.timing.finalized_at = None;
        let stored = store.save_observation(&without_finalized, None).unwrap();
        assert_eq!(
            store
                .load_observation(&without_finalized.observation_id)
                .unwrap()
                .unwrap(),
            stored
        );
        assert_eq!(
            store
                .list_terminal_history(ObservationQuery::new(10))
                .unwrap()
                .observations
                .len(),
            1
        );

        let mut provisional = without_finalized;
        provisional.observation_id = "obs-provisional-finalized".to_owned();
        provisional.lifecycle_state = ObservationLifecycle::Reconciling;
        provisional.timing.finalized_at = Some("2026-10-04T10:00:00Z".to_owned());
        assert!(matches!(
            store.save_observation(&provisional, None),
            Err(StorageError::ObservationCorrupt)
        ));
    }

    #[test]
    fn duration_must_match_complete_task_interval() {
        let template = fixture("observation-codex-finalized.json");
        let mut store = SqliteStore::open_in_memory().unwrap();
        let mut mismatch = template.clone();
        mismatch.observation_id = "obs-duration-mismatch".to_owned();
        mismatch.timing.duration_ms = Some(999);
        assert!(matches!(
            store.save_observation(&mismatch, None),
            Err(StorageError::ObservationCorrupt)
        ));
        let mut missing = template.clone();
        missing.observation_id = "obs-duration-missing".to_owned();
        missing.timing.duration_ms = None;
        assert!(matches!(
            store.save_observation(&missing, None),
            Err(StorageError::ObservationCorrupt)
        ));
        let mut no_start = template;
        no_start.observation_id = "obs-duration-no-start".to_owned();
        no_start.timing.started_at = None;
        no_start.timing.duration_ms = Some(1);
        assert!(matches!(
            store.save_observation(&no_start, None),
            Err(StorageError::ObservationCorrupt)
        ));
    }

    #[test]
    fn exact_composite_replay_is_a_true_noop_and_partial_replays_progress_one_side() {
        let observation = fixture("observation-codex-finalized.json");
        let source = crate::telemetry::SourceIdentity::new("rollout-replay", "generation").unwrap();
        let cursor = crate::telemetry::RolloutCursor::at_start(&source);
        let mut store = SqliteStore::open_in_memory().unwrap();
        let first = store
            .commit_observation_and_checkpoint(
                &observation,
                None,
                &source,
                &cursor,
                &crate::storage::RuntimeRecoveryState::default(),
                None,
            )
            .unwrap();
        let replay = store
            .commit_observation_and_checkpoint(
                &observation,
                Some(999),
                &source,
                &cursor,
                &crate::storage::RuntimeRecoveryState::default(),
                Some(999),
            )
            .unwrap();
        assert_eq!(first.0.storage_revision, replay.0.storage_revision);
        assert_eq!(first.1.revision, replay.1.revision);

        let advanced_cursor = crate::telemetry::RolloutCursor::from_checkpoint(&source, 10, None);
        let advanced = store
            .commit_observation_and_checkpoint(
                &observation,
                Some(999),
                &source,
                &advanced_cursor,
                &crate::storage::RuntimeRecoveryState::default(),
                Some(1),
            )
            .unwrap();
        assert_eq!(advanced.0.storage_revision, 1);
        assert_eq!(advanced.1.revision, 2);

        let mut provisional = observation.clone();
        provisional.observation_id = "obs-provisional-composite".to_owned();
        provisional.lifecycle_state = ObservationLifecycle::AwaitingMeter;
        provisional.timing.finalized_at = None;
        let initial = store
            .commit_observation_and_checkpoint(
                &provisional,
                None,
                &source,
                &advanced_cursor,
                &crate::storage::RuntimeRecoveryState::default(),
                Some(2),
            )
            .unwrap();
        let mut progressed = provisional;
        progressed.lifecycle_state = ObservationLifecycle::Reconciling;
        let progressed = store
            .commit_observation_and_checkpoint(
                &progressed,
                Some(initial.0.storage_revision),
                &source,
                &advanced_cursor,
                &crate::storage::RuntimeRecoveryState::default(),
                Some(999),
            )
            .unwrap();
        assert_eq!(progressed.0.storage_revision, 2);
        assert_eq!(progressed.1.revision, initial.1.revision);
    }

    #[test]
    fn stale_non_identical_checkpoint_rolls_back_observation() {
        let observation = fixture("observation-codex-finalized.json");
        let source = crate::telemetry::SourceIdentity::new("rollout-stale", "generation").unwrap();
        let cursor = crate::telemetry::RolloutCursor::at_start(&source);
        let mut store = SqliteStore::open_in_memory().unwrap();
        store
            .save_runtime_checkpoint(
                &source,
                &cursor,
                &crate::storage::RuntimeRecoveryState::default(),
                None,
            )
            .unwrap();
        let mut changed = observation.clone();
        changed.observation_id = "obs-stale-rollback".to_owned();
        assert!(matches!(
            store.commit_observation_and_checkpoint(
                &changed,
                None,
                &source,
                &crate::telemetry::RolloutCursor::from_checkpoint(&source, 1, None),
                &crate::storage::RuntimeRecoveryState::default(),
                Some(99)
            ),
            Err(StorageError::CheckpointRevisionConflict { .. })
        ));
        assert!(store
            .load_observation(&changed.observation_id)
            .unwrap()
            .is_none());
    }

    #[test]
    fn corrupt_checkpoint_cannot_qualify_as_exact_replay() {
        let observation = fixture("observation-codex-finalized.json");
        let source =
            crate::telemetry::SourceIdentity::new("rollout-corrupt", "generation").unwrap();
        let cursor = crate::telemetry::RolloutCursor::at_start(&source);
        let mut store = SqliteStore::open_in_memory().unwrap();
        store
            .commit_observation_and_checkpoint(
                &observation,
                None,
                &source,
                &cursor,
                &crate::storage::RuntimeRecoveryState::default(),
                None,
            )
            .unwrap();
        store
            .connection()
            .execute(
                "UPDATE runtime_checkpoints SET state_sha256 = ?1",
                ["0".repeat(64)],
            )
            .unwrap();
        assert!(matches!(
            store.commit_observation_and_checkpoint(
                &observation,
                Some(999),
                &source,
                &cursor,
                &crate::storage::RuntimeRecoveryState::default(),
                Some(999),
            ),
            Err(StorageError::CheckpointCorrupt)
        ));
    }
}
