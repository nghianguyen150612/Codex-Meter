use std::collections::BTreeMap;

use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use time::Duration;

use crate::telemetry::{
    QuotaTrackingState, ReconciliationPolicy, RolloutCursor, SourceIdentity,
    TaskQuotaReconciliation, TelemetryState,
};

use super::sqlite::StorageError;

pub const STATE_FORMAT_VERSION: u32 = 1;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeCheckpoint {
    pub source: SourceIdentity,
    pub cursor: RolloutCursor,
    pub state: RuntimeRecoveryState,
    pub revision: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RuntimeRecoveryState {
    pub telemetry_state: TelemetryState,
    pub quota_tracking_state: QuotaTrackingState,
    pub active_reconciliations: BTreeMap<String, ActiveReconciliationCheckpoint>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActiveReconciliationCheckpoint {
    pub policy: ReconciliationPolicy,
    pub reconciliation: TaskQuotaReconciliation,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PersistedRuntimeStateV1 {
    state_format_version: u32,
    telemetry_state: TelemetryState,
    quota_tracking_state: QuotaTrackingState,
    active_reconciliations: BTreeMap<String, PersistedActiveReconciliationV1>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PersistedActiveReconciliationV1 {
    policy: PersistedReconciliationPolicyV1,
    reconciliation: TaskQuotaReconciliation,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PersistedReconciliationPolicyV1 {
    max_before_sample_age: PersistedDurationV1,
    post_task_sample_offsets: Vec<PersistedDurationV1>,
    stabilization_not_before: PersistedDurationV1,
    required_stable_confirmations: usize,
    deadline: PersistedDurationV1,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PersistedDurationV1 {
    seconds: i64,
    nanoseconds: i32,
}

impl From<Duration> for PersistedDurationV1 {
    fn from(value: Duration) -> Self {
        Self {
            seconds: value.whole_seconds(),
            nanoseconds: value.subsec_nanoseconds(),
        }
    }
}

impl TryFrom<PersistedDurationV1> for Duration {
    type Error = ();

    fn try_from(value: PersistedDurationV1) -> Result<Self, Self::Error> {
        if !(-999_999_999..=999_999_999).contains(&value.nanoseconds) {
            return Err(());
        }
        Ok(Self::new(value.seconds, value.nanoseconds))
    }
}

impl From<&ReconciliationPolicy> for PersistedReconciliationPolicyV1 {
    fn from(value: &ReconciliationPolicy) -> Self {
        Self {
            max_before_sample_age: value.max_before_sample_age.into(),
            post_task_sample_offsets: value
                .post_task_sample_offsets
                .iter()
                .copied()
                .map(Into::into)
                .collect(),
            stabilization_not_before: value.stabilization_not_before.into(),
            required_stable_confirmations: value.required_stable_confirmations,
            deadline: value.deadline.into(),
        }
    }
}

impl TryFrom<PersistedReconciliationPolicyV1> for ReconciliationPolicy {
    type Error = ();

    fn try_from(value: PersistedReconciliationPolicyV1) -> Result<Self, Self::Error> {
        let policy = Self {
            max_before_sample_age: value.max_before_sample_age.try_into()?,
            post_task_sample_offsets: value
                .post_task_sample_offsets
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<Vec<_>, _>>()?,
            stabilization_not_before: value.stabilization_not_before.try_into()?,
            required_stable_confirmations: value.required_stable_confirmations,
            deadline: value.deadline.try_into()?,
        };
        policy.validate().map_err(|_| ())?;
        Ok(policy)
    }
}

impl From<&RuntimeRecoveryState> for PersistedRuntimeStateV1 {
    fn from(value: &RuntimeRecoveryState) -> Self {
        Self {
            state_format_version: STATE_FORMAT_VERSION,
            telemetry_state: value.telemetry_state.clone(),
            quota_tracking_state: value.quota_tracking_state.clone(),
            active_reconciliations: value
                .active_reconciliations
                .iter()
                .map(|(task_id, checkpoint)| {
                    (
                        task_id.clone(),
                        PersistedActiveReconciliationV1 {
                            policy: (&checkpoint.policy).into(),
                            reconciliation: checkpoint.reconciliation.clone(),
                        },
                    )
                })
                .collect(),
        }
    }
}

impl TryFrom<PersistedRuntimeStateV1> for RuntimeRecoveryState {
    type Error = StorageError;

    fn try_from(value: PersistedRuntimeStateV1) -> Result<Self, Self::Error> {
        if value.state_format_version > STATE_FORMAT_VERSION {
            return Err(StorageError::CheckpointFormatTooNew {
                found: value.state_format_version,
                supported: STATE_FORMAT_VERSION,
            });
        }
        if value.state_format_version == 0 {
            return Err(StorageError::CheckpointFormatUnsupported { found: 0 });
        }
        let mut active_reconciliations = BTreeMap::new();
        for (task_id, persisted) in value.active_reconciliations {
            let policy = ReconciliationPolicy::try_from(persisted.policy)
                .map_err(|_| StorageError::CheckpointStateInvalid)?;
            if task_id != persisted.reconciliation.task.task_id {
                return Err(StorageError::CheckpointStateInvalid);
            }
            persisted
                .reconciliation
                .validate_recovered(&policy)
                .map_err(|_| StorageError::CheckpointStateInvalid)?;
            active_reconciliations.insert(
                task_id,
                ActiveReconciliationCheckpoint {
                    policy,
                    reconciliation: persisted.reconciliation,
                },
            );
        }
        validate_telemetry(&value.telemetry_state)?;
        value
            .quota_tracking_state
            .five_hour
            .validate_recovered()
            .map_err(|_| StorageError::CheckpointStateInvalid)?;
        value
            .quota_tracking_state
            .weekly
            .validate_recovered()
            .map_err(|_| StorageError::CheckpointStateInvalid)?;
        Ok(Self {
            telemetry_state: value.telemetry_state,
            quota_tracking_state: value.quota_tracking_state,
            active_reconciliations,
        })
    }
}

fn validate_telemetry(state: &TelemetryState) -> Result<(), StorageError> {
    for (task_id, task) in &state.tasks {
        if task_id.is_empty() || task.task_id != *task_id {
            return Err(StorageError::CheckpointStateInvalid);
        }
    }
    if let Some(session) = &state.session {
        if session.session_id.is_empty()
            || session.thread_id.is_empty()
            || session.cli_version.is_empty()
        {
            return Err(StorageError::CheckpointStateInvalid);
        }
    }
    Ok(())
}

fn encode_state(state: &RuntimeRecoveryState) -> Result<(String, String), StorageError> {
    validate_runtime_state(state)?;
    let dto = PersistedRuntimeStateV1::from(state);
    let bytes = serde_json::to_vec(&dto).map_err(|_| StorageError::CheckpointDecode)?;
    let json = String::from_utf8(bytes).map_err(|_| StorageError::CheckpointDecode)?;
    let checksum = Sha256::digest(json.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Ok((json, checksum))
}

fn validate_runtime_state(state: &RuntimeRecoveryState) -> Result<(), StorageError> {
    validate_telemetry(&state.telemetry_state)?;
    state
        .quota_tracking_state
        .five_hour
        .validate_recovered()
        .map_err(|_| StorageError::CheckpointStateInvalid)?;
    state
        .quota_tracking_state
        .weekly
        .validate_recovered()
        .map_err(|_| StorageError::CheckpointStateInvalid)?;
    for (task_id, active) in &state.active_reconciliations {
        if task_id != &active.reconciliation.task.task_id {
            return Err(StorageError::CheckpointStateInvalid);
        }
        active
            .reconciliation
            .validate_recovered(&active.policy)
            .map_err(|_| StorageError::CheckpointStateInvalid)?;
    }
    Ok(())
}

fn decode_state(json: &str, checksum: &str) -> Result<RuntimeRecoveryState, StorageError> {
    let calculated = Sha256::digest(json.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    if calculated != checksum {
        return Err(StorageError::CheckpointCorrupt);
    }
    let dto: PersistedRuntimeStateV1 =
        serde_json::from_str(json).map_err(|_| StorageError::CheckpointDecode)?;
    dto.try_into()
}

pub(crate) fn load_runtime_checkpoint(
    connection: &Connection,
    requested_source: &SourceIdentity,
) -> Result<Option<RuntimeCheckpoint>, StorageError> {
    let row = connection
        .query_row(
            "SELECT rollout_id, source_generation, committed_offset, last_ordinal, checkpoint_revision, state_format_version, state_json, state_sha256 FROM runtime_checkpoints WHERE rollout_id = ?1 AND source_generation = ?2",
            params![requested_source.rollout_id(), requested_source.source_generation()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                ))
            },
        )
        .optional()
        .map_err(StorageError::sqlite)?;
    let Some((rollout_id, source_generation, offset, ordinal, revision, format, json, checksum)) =
        row
    else {
        return Ok(None);
    };
    let source = SourceIdentity::new(rollout_id, source_generation)
        .map_err(|_| StorageError::CheckpointCursorInvalid)?;
    let committed_offset = parse_u64(&offset)?;
    let last_ordinal = ordinal.as_deref().map(parse_u64).transpose()?;
    let revision = u64::try_from(revision).map_err(|_| StorageError::CheckpointCorrupt)?;
    let format = u32::try_from(format)
        .map_err(|_| StorageError::CheckpointFormatUnsupported { found: 0 })?;
    if format > STATE_FORMAT_VERSION {
        return Err(StorageError::CheckpointFormatTooNew {
            found: format,
            supported: STATE_FORMAT_VERSION,
        });
    }
    if format == 0 {
        return Err(StorageError::CheckpointFormatUnsupported { found: format });
    }
    let state = decode_state(&json, &checksum)?;
    let cursor = RolloutCursor::from_checkpoint(&source, committed_offset, last_ordinal);
    if source != *requested_source {
        return Err(StorageError::CheckpointSourceMismatch);
    }
    Ok(Some(RuntimeCheckpoint {
        source,
        cursor,
        state,
        revision,
    }))
}

pub(crate) fn save_runtime_checkpoint(
    connection: &mut Connection,
    source: &SourceIdentity,
    cursor: &RolloutCursor,
    state: &RuntimeRecoveryState,
    expected_revision: Option<u64>,
) -> Result<RuntimeCheckpoint, StorageError> {
    if source.rollout_id() != cursor.rollout_id()
        || source.source_generation() != cursor.source_generation()
    {
        return Err(StorageError::CheckpointSourceMismatch);
    }
    let (state_json, state_sha256) = encode_state(state)?;
    let transaction = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(StorageError::sqlite)?;
    let result = save_in_transaction(
        &transaction,
        source,
        cursor,
        state,
        state_json,
        state_sha256,
        expected_revision,
    );
    match result {
        Ok(checkpoint) => {
            transaction.commit().map_err(StorageError::sqlite)?;
            Ok(checkpoint)
        }
        Err(error) => Err(error),
    }
}

fn save_in_transaction(
    transaction: &Transaction<'_>,
    source: &SourceIdentity,
    cursor: &RolloutCursor,
    state: &RuntimeRecoveryState,
    state_json: String,
    state_sha256: String,
    expected_revision: Option<u64>,
) -> Result<RuntimeCheckpoint, StorageError> {
    let actual_revision = transaction
        .query_row(
            "SELECT checkpoint_revision FROM runtime_checkpoints WHERE rollout_id = ?1 AND source_generation = ?2",
            params![source.rollout_id(), source.source_generation()],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(StorageError::sqlite)?
        .map(|value| u64::try_from(value).map_err(|_| StorageError::CheckpointCorrupt))
        .transpose()?;
    let revision = match (expected_revision, actual_revision) {
        (None, Some(actual)) => {
            return Err(StorageError::CheckpointRevisionConflict {
                expected: None,
                actual: Some(actual),
            })
        }
        (Some(expected), Some(actual)) if expected != actual => {
            return Err(StorageError::CheckpointRevisionConflict {
                expected: Some(expected),
                actual: Some(actual),
            })
        }
        (Some(_), None) | (None, None) => 1,
        (Some(expected), Some(_)) => expected
            .checked_add(1)
            .ok_or(StorageError::CheckpointRevisionOverflow)?,
    };
    let revision_i64 =
        i64::try_from(revision).map_err(|_| StorageError::CheckpointRevisionOverflow)?;
    let expected_revision_i64 = expected_revision
        .map(i64::try_from)
        .transpose()
        .map_err(|_| StorageError::CheckpointRevisionOverflow)?;
    let offset = cursor.committed_offset().to_string();
    let ordinal = cursor.last_ordinal().map(|value| value.to_string());
    if expected_revision.is_none() {
        transaction
            .execute(
                "INSERT INTO runtime_checkpoints (rollout_id, source_generation, committed_offset, last_ordinal, checkpoint_revision, state_format_version, state_json, state_sha256) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![source.rollout_id(), source.source_generation(), offset, ordinal, revision_i64, STATE_FORMAT_VERSION, state_json, state_sha256],
            )
            .map_err(StorageError::sqlite)?;
    } else {
        let changed = transaction
            .execute(
                "UPDATE runtime_checkpoints SET committed_offset = ?1, last_ordinal = ?2, checkpoint_revision = ?3, state_format_version = ?4, state_json = ?5, state_sha256 = ?6 WHERE rollout_id = ?7 AND source_generation = ?8 AND checkpoint_revision = ?9",
                params![offset, ordinal, revision_i64, STATE_FORMAT_VERSION, state_json, state_sha256, source.rollout_id(), source.source_generation(), expected_revision_i64],
            )
            .map_err(StorageError::sqlite)?;
        if changed != 1 {
            return Err(StorageError::CheckpointRevisionConflict {
                expected: expected_revision,
                actual: actual_revision,
            });
        }
    }
    Ok(RuntimeCheckpoint {
        source: source.clone(),
        cursor: cursor.clone(),
        state: state.clone(),
        revision,
    })
}

fn parse_u64(value: &str) -> Result<u64, StorageError> {
    value
        .parse::<u64>()
        .map_err(|_| StorageError::CheckpointCursorInvalid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::SqliteStore;
    use crate::telemetry::{
        advance_meter, AcquisitionStatus, ConfigurationConsistency, ConfigurationIdentity,
        ConfigurationState, ConfigurationValue, MetricAvailability, MetricProvenance,
        NormalizedQuotaSample, PercentageMetric, QuotaMeterType, QuotaSourceKind,
        QuotaWindowIdentity, SchemaVersion, SessionContext, TaskAnomaly, TaskLifecycle, TaskState,
        TaskTerminalEvidence, TelemetryState,
    };
    use rusqlite::params;
    use std::collections::{BTreeMap, BTreeSet};
    fn source(generation: &str) -> SourceIdentity {
        SourceIdentity::new("checkpoint-rollout", generation).unwrap()
    }

    fn unavailable() -> ConfigurationValue {
        ConfigurationValue {
            availability: MetricAvailability::Unavailable,
            value: None,
            provenance: MetricProvenance::Unavailable,
        }
    }

    fn sample(id: &str, meter_type: QuotaMeterType, value: f64) -> NormalizedQuotaSample {
        NormalizedQuotaSample {
            schema_version: SchemaVersion::V1,
            sample_id: id.to_owned(),
            meter_type,
            sampled_at: "2026-10-04T10:00:00Z".to_owned(),
            used_percent: PercentageMetric {
                availability: MetricAvailability::Available,
                value: Some(value),
                provenance: MetricProvenance::Observed,
            },
            remaining_percent: PercentageMetric {
                availability: MetricAvailability::Available,
                value: Some(100.0 - value),
                provenance: MetricProvenance::Derived,
            },
            reset_evidence: QuotaWindowIdentity::Unavailable,
            configuration: ConfigurationIdentity {
                plan: unavailable(),
                model: unavailable(),
                reasoning_level: unavailable(),
                speed_mode: unavailable(),
                codex_version: unavailable(),
            },
            acquisition_status: AcquisitionStatus::Succeeded,
            source_kind: QuotaSourceKind::LocalMeter,
        }
    }

    #[test]
    fn duration_round_trip_is_exact() {
        let duration = Duration::new(-4, -123_456_789);
        let encoded = PersistedDurationV1::from(duration);
        assert_eq!(Duration::try_from(encoded), Ok(duration));
    }

    #[test]
    fn checkpoint_round_trip_preserves_full_u64_cursor_domain() {
        let mut store = SqliteStore::open_in_memory().unwrap();
        let source = source("generation-1");
        let cursor = RolloutCursor::from_checkpoint(&source, u64::MAX, Some(0));
        let saved = store
            .save_runtime_checkpoint(&source, &cursor, &RuntimeRecoveryState::default(), None)
            .unwrap();
        assert_eq!(saved.revision, 1);
        let loaded = store.load_runtime_checkpoint(&source).unwrap().unwrap();
        assert_eq!(loaded.cursor, cursor);
        assert_eq!(loaded.state, RuntimeRecoveryState::default());
    }

    #[test]
    fn revision_cas_and_generation_isolation_are_enforced() {
        let mut store = SqliteStore::open_in_memory().unwrap();
        let first_source = source("generation-1");
        let second_source = source("generation-2");
        let first = RolloutCursor::from_checkpoint(&first_source, 100, None);
        let second = RolloutCursor::from_checkpoint(&second_source, 10, None);
        store
            .save_runtime_checkpoint(
                &first_source,
                &first,
                &RuntimeRecoveryState::default(),
                None,
            )
            .unwrap();
        store
            .save_runtime_checkpoint(
                &second_source,
                &second,
                &RuntimeRecoveryState::default(),
                None,
            )
            .unwrap();
        let updated = RolloutCursor::from_checkpoint(&first_source, 101, Some(7));
        assert_eq!(
            store
                .save_runtime_checkpoint(
                    &first_source,
                    &updated,
                    &RuntimeRecoveryState::default(),
                    Some(1),
                )
                .unwrap()
                .revision,
            2
        );
        assert!(matches!(
            store.save_runtime_checkpoint(
                &first_source,
                &first,
                &RuntimeRecoveryState::default(),
                Some(1),
            ),
            Err(StorageError::CheckpointRevisionConflict { .. })
        ));
        assert_eq!(
            store
                .load_runtime_checkpoint(&second_source)
                .unwrap()
                .unwrap()
                .cursor,
            second
        );
    }

    #[test]
    fn load_missing_checkpoint_is_read_only() {
        let store = SqliteStore::open_in_memory().unwrap();
        let source = source("generation-1");
        assert!(store.load_runtime_checkpoint(&source).unwrap().is_none());
        let count: i64 = store
            .connection()
            .query_row("SELECT count(*) FROM runtime_checkpoints", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn quota_replay_ids_survive_checkpoint_restore() {
        let mut store = SqliteStore::open_in_memory().unwrap();
        let source = source("generation-1");
        let quota_sample = sample("sample-1", QuotaMeterType::FiveHour, 25.0);
        let mut state = QuotaTrackingState::default();
        advance_meter(&mut state.five_hour, &quota_sample).unwrap();
        let recovery = RuntimeRecoveryState {
            quota_tracking_state: state,
            ..RuntimeRecoveryState::default()
        };
        store
            .save_runtime_checkpoint(&source, &RolloutCursor::at_start(&source), &recovery, None)
            .unwrap();
        let restored = store.load_runtime_checkpoint(&source).unwrap().unwrap();
        let outcome = advance_meter(
            &mut restored.state.quota_tracking_state.five_hour.clone(),
            &quota_sample,
        )
        .unwrap();
        assert!(matches!(
            outcome,
            crate::telemetry::QuotaTrackingOutcome::DuplicateSample { .. }
        ));
    }

    #[test]
    fn checksum_and_decode_fail_closed() {
        let mut store = SqliteStore::open_in_memory().unwrap();
        let source = source("generation-1");
        store
            .save_runtime_checkpoint(
                &source,
                &RolloutCursor::at_start(&source),
                &RuntimeRecoveryState::default(),
                None,
            )
            .unwrap();
        store
            .connection()
            .execute(
                "UPDATE runtime_checkpoints SET state_sha256 = ?1",
                params!["0".repeat(64)],
            )
            .unwrap();
        assert!(matches!(
            store.load_runtime_checkpoint(&source),
            Err(StorageError::CheckpointCorrupt)
        ));
    }

    #[test]
    fn format_and_unknown_payloads_are_rejected() {
        let mut store = SqliteStore::open_in_memory().unwrap();
        let source = source("generation-1");
        store
            .save_runtime_checkpoint(
                &source,
                &RolloutCursor::at_start(&source),
                &RuntimeRecoveryState::default(),
                None,
            )
            .unwrap();
        let json: String = store
            .connection()
            .query_row("SELECT state_json FROM runtime_checkpoints", [], |row| {
                row.get(0)
            })
            .unwrap();
        let mut value: serde_json::Value = serde_json::from_str(&json).unwrap();
        value["unexpected"] = serde_json::Value::Bool(true);
        let modified = serde_json::to_string(&value).unwrap();
        let checksum = Sha256::digest(modified.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        store
            .connection()
            .execute(
                "UPDATE runtime_checkpoints SET state_json = ?1, state_sha256 = ?2",
                params![modified, checksum],
            )
            .unwrap();
        assert!(matches!(
            store.load_runtime_checkpoint(&source),
            Err(StorageError::CheckpointDecode)
        ));
    }

    #[test]
    fn newer_format_and_malformed_cursor_are_rejected() {
        let mut store = SqliteStore::open_in_memory().unwrap();
        let source = source("generation-1");
        store
            .save_runtime_checkpoint(
                &source,
                &RolloutCursor::at_start(&source),
                &RuntimeRecoveryState::default(),
                None,
            )
            .unwrap();
        store
            .connection()
            .execute(
                "UPDATE runtime_checkpoints SET state_format_version = 999",
                [],
            )
            .unwrap();
        assert!(matches!(
            store.load_runtime_checkpoint(&source),
            Err(StorageError::CheckpointFormatTooNew { found: 999, .. })
        ));
        let (json, checksum): (String, String) = store
            .connection()
            .query_row(
                "SELECT state_json, state_sha256 FROM runtime_checkpoints",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        store
            .connection()
            .execute("DROP TABLE runtime_checkpoints", [])
            .unwrap();
        store
            .connection()
            .execute_batch(
                "CREATE TABLE runtime_checkpoints (rollout_id TEXT, source_generation TEXT, committed_offset TEXT, last_ordinal TEXT, checkpoint_revision INTEGER, state_format_version INTEGER, state_json TEXT, state_sha256 TEXT, PRIMARY KEY (rollout_id, source_generation));",
            )
            .unwrap();
        store
            .connection()
            .execute(
                "INSERT INTO runtime_checkpoints VALUES (?1, ?2, ?3, NULL, 1, 1, ?4, ?5)",
                params![
                    source.rollout_id(),
                    source.source_generation(),
                    " 12",
                    json,
                    checksum
                ],
            )
            .unwrap();
        assert!(matches!(
            store.load_runtime_checkpoint(&source),
            Err(StorageError::CheckpointCursorInvalid)
        ));
    }

    #[test]
    fn fixed_default_state_checksum_is_stable() {
        let (_, checksum) = encode_state(&RuntimeRecoveryState::default()).unwrap();
        assert_eq!(
            checksum,
            "62981bef0c6ccdfa5afaaf020ccf8298f8fddb9a9ba0fc5ba8651c68e1954fa7"
        );
    }

    #[test]
    fn telemetry_and_reconciliation_state_round_trip() {
        let source = source("generation-1");
        let mut telemetry = TelemetryState {
            session: Some(SessionContext {
                session_id: "session-1".to_owned(),
                thread_id: "thread-1".to_owned(),
                cli_version: "0.157.1".to_owned(),
                model_provider: Some("provider".to_owned()),
            }),
            thread_configuration: ConfigurationState {
                model: Some("model".to_owned()),
                ..ConfigurationState::default()
            },
            tasks: BTreeMap::new(),
        };
        telemetry.tasks.insert(
            "task-1".to_owned(),
            TaskState {
                task_id: "task-1".to_owned(),
                root_task_id: Some("task-1".to_owned()),
                observed_root_task_ids: ["task-1".to_owned()].into_iter().collect(),
                lifecycle: TaskLifecycle::Active,
                start_observed: true,
                terminal_evidence: BTreeSet::<TaskTerminalEvidence>::new(),
                anomalies: BTreeSet::<TaskAnomaly>::new(),
                configuration_override: ConfigurationState::default(),
                token_configuration_fingerprints: BTreeSet::new(),
                configuration_consistency: ConfigurationConsistency::Consistent,
            },
        );
        let policy = ReconciliationPolicy {
            max_before_sample_age: Duration::minutes(30),
            post_task_sample_offsets: vec![Duration::seconds(10), Duration::seconds(20)],
            stabilization_not_before: Duration::seconds(10),
            required_stable_confirmations: 2,
            deadline: Duration::seconds(30),
        };
        let baseline = sample("baseline", QuotaMeterType::FiveHour, 20.0);
        let reconciliation = crate::telemetry::begin_task_quota_reconciliation(
            crate::telemetry::TaskReconciliationTarget::new(
                "task-1",
                Some("2026-10-04T10:01:00Z".to_owned()),
                Some("2026-10-04T10:02:00Z".to_owned()),
            ),
            &[baseline],
            &policy,
        )
        .unwrap();
        let state = RuntimeRecoveryState {
            telemetry_state: telemetry,
            active_reconciliations: [(
                "task-1".to_owned(),
                ActiveReconciliationCheckpoint {
                    policy,
                    reconciliation,
                },
            )]
            .into_iter()
            .collect(),
            ..RuntimeRecoveryState::default()
        };
        let mut store = SqliteStore::open_in_memory().unwrap();
        store
            .save_runtime_checkpoint(&source, &RolloutCursor::at_start(&source), &state, None)
            .unwrap();
        assert_eq!(
            store
                .load_runtime_checkpoint(&source)
                .unwrap()
                .unwrap()
                .state,
            state
        );
    }
}
