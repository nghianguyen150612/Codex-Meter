use std::collections::{BTreeMap, BTreeSet};

use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use time::Duration;

use crate::telemetry::task_quota_sampling::StoredSamplingEvent;
use crate::telemetry::{
    begin_task_quota_reconciliation, reconcile_task_quota_at, NormalizedQuotaSample,
    QuotaAcquisitionResult, QuotaTrackingState, ReconciliationPolicy, RolloutCursor,
    SourceIdentity, TaskQuotaReconciliation, TaskReconciliationTarget, TelemetryState,
};

use super::sqlite::StorageError;

/// Version 3 adds non-authoritative sampling association metadata. Versions 1 and
/// 2 remain readable; absent metadata is always treated as unavailable.
pub const STATE_FORMAT_VERSION: u32 = 3;
const MAX_RUNTIME_STATE_BYTES: usize = 4 * 1024 * 1024;
const MAX_QUALIFIED_WORKFLOWS: usize = 128;
const MAX_SAMPLES_PER_WORKFLOW: usize = 512;
const MAX_CHECKPOINT_IDENTIFIER_BYTES: usize = 256;

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
    /// Persisted association and replay metadata only. This never represents a
    /// source credential; recovery requires a fresh live authority check.
    pub qualified_sampling: BTreeMap<String, QualifiedSamplingCheckpoint>,
}

/// Privacy-safe durable metadata for one qualified sampling workflow. Sample
/// payloads and authority capabilities are intentionally excluded.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct QualifiedSamplingCheckpoint {
    pub source_generation_id: String,
    pub task_id: String,
    pub session_id: Option<String>,
    pub started_at: String,
    pub ended_at: String,
    pub contract_version: u32,
    pub authority_contract: String,
    pub phase: QualifiedSamplingPhase,
    #[serde(with = "reconciliation_policy_serde")]
    pub policy: ReconciliationPolicy,
    pub original_deadline_at: String,
    pub last_evaluation_at: Option<String>,
    /// Keys are `meter:sample-id`; values are lowercase SHA-256 payload digests.
    pub accepted_sample_digests: BTreeMap<String, String>,
    /// True when `active_reconciliations[task_id]` contains the P012 state.
    pub reconciliation_initialized: bool,
    /// Normalized pre-task evidence retained until P012 chooses each baseline.
    #[serde(default)]
    pub baseline_samples: Vec<NormalizedQuotaSample>,
    /// Accepted normalized events, including events P012 has not yet been able to apply.
    #[serde(default)]
    pub accepted_events: Vec<StoredSamplingEvent>,
    #[serde(default)]
    pub known_overlapping_task_ids: std::collections::BTreeSet<String>,
}

impl QualifiedSamplingCheckpoint {
    /// Stable, collision resistant workflow key preserving task, session,
    /// interval and source-generation identity.
    pub fn workflow_id(&self) -> String {
        let identity = serde_json::to_vec(&(
            self.contract_version,
            &self.source_generation_id,
            &self.task_id,
            &self.session_id,
            &self.started_at,
            &self.ended_at,
        ))
        .expect("serializing a tuple of strings cannot fail");
        Sha256::digest(identity)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QualifiedSamplingPhase {
    Association,
    BeforeTask,
    Reconciling,
    Terminal,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActiveReconciliationCheckpoint {
    pub policy: ReconciliationPolicy,
    pub reconciliation: TaskQuotaReconciliation,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PersistedRuntimeState {
    state_format_version: u32,
    telemetry_state: TelemetryState,
    quota_tracking_state: QuotaTrackingState,
    active_reconciliations: BTreeMap<String, PersistedActiveReconciliationV1>,
    #[serde(default)]
    qualified_sampling: BTreeMap<String, QualifiedSamplingCheckpoint>,
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

mod reconciliation_policy_serde {
    use super::{PersistedReconciliationPolicyV1, ReconciliationPolicy};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S>(value: &ReconciliationPolicy, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        PersistedReconciliationPolicyV1::from(value).serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<ReconciliationPolicy, D::Error>
    where
        D: Deserializer<'de>,
    {
        PersistedReconciliationPolicyV1::deserialize(deserializer)?
            .try_into()
            .map_err(|_| serde::de::Error::custom("invalid reconciliation policy"))
    }
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

impl From<&RuntimeRecoveryState> for PersistedRuntimeState {
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
            qualified_sampling: value.qualified_sampling.clone(),
        }
    }
}

impl TryFrom<PersistedRuntimeState> for RuntimeRecoveryState {
    type Error = StorageError;

    fn try_from(value: PersistedRuntimeState) -> Result<Self, Self::Error> {
        if value.state_format_version > STATE_FORMAT_VERSION {
            return Err(StorageError::CheckpointFormatTooNew {
                found: value.state_format_version,
                supported: STATE_FORMAT_VERSION,
            });
        }
        if value.state_format_version == 0 {
            return Err(StorageError::CheckpointFormatUnsupported { found: 0 });
        }
        if value.state_format_version < 3 && !value.qualified_sampling.is_empty() {
            return Err(StorageError::CheckpointStateInvalid);
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
        validate_qualified_sampling(&value.qualified_sampling, &active_reconciliations)?;
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
            qualified_sampling: value.qualified_sampling,
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

pub(crate) fn encode_state(state: &RuntimeRecoveryState) -> Result<(String, String), StorageError> {
    validate_runtime_state(state)?;
    let dto = PersistedRuntimeState::from(state);
    let bytes = serde_json::to_vec(&dto).map_err(|_| StorageError::CheckpointDecode)?;
    if bytes.len() > MAX_RUNTIME_STATE_BYTES {
        return Err(StorageError::CheckpointStateInvalid);
    }
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
    validate_qualified_sampling(&state.qualified_sampling, &state.active_reconciliations)?;
    Ok(())
}

fn validate_qualified_sampling(
    workflows: &BTreeMap<String, QualifiedSamplingCheckpoint>,
    reconciliations: &BTreeMap<String, ActiveReconciliationCheckpoint>,
) -> Result<(), StorageError> {
    if workflows.len() > MAX_QUALIFIED_WORKFLOWS {
        return Err(StorageError::CheckpointStateInvalid);
    }
    for (workflow_id, workflow) in workflows {
        if workflow_id.is_empty()
            || workflow_id.len() > MAX_CHECKPOINT_IDENTIFIER_BYTES
            || workflow.task_id.is_empty()
            || workflow.task_id.len() > MAX_CHECKPOINT_IDENTIFIER_BYTES
            || workflow.source_generation_id.is_empty()
            || workflow.source_generation_id.len() > MAX_CHECKPOINT_IDENTIFIER_BYTES
            || workflow.authority_contract.is_empty()
            || workflow.authority_contract.len() > MAX_CHECKPOINT_IDENTIFIER_BYTES
            || workflow.contract_version != 1
            || workflow
                .session_id
                .as_ref()
                .is_some_and(|v| v.is_empty() || v.len() > MAX_CHECKPOINT_IDENTIFIER_BYTES)
            || workflow.accepted_sample_digests.len() > MAX_SAMPLES_PER_WORKFLOW
            || workflow.accepted_events.len() > MAX_SAMPLES_PER_WORKFLOW
            || workflow.baseline_samples.len() > MAX_SAMPLES_PER_WORKFLOW
            || workflow
                .accepted_events
                .len()
                .saturating_add(workflow.baseline_samples.len())
                > MAX_SAMPLES_PER_WORKFLOW
            || workflow.known_overlapping_task_ids.len() > MAX_SAMPLES_PER_WORKFLOW
            || workflow_id != &workflow.workflow_id()
        {
            return Err(StorageError::CheckpointStateInvalid);
        }
        workflow
            .policy
            .validate()
            .map_err(|_| StorageError::CheckpointStateInvalid)?;
        if workflow
            .known_overlapping_task_ids
            .iter()
            .any(|id| id.is_empty() || id.len() > MAX_CHECKPOINT_IDENTIFIER_BYTES)
        {
            return Err(StorageError::CheckpointStateInvalid);
        }
        validate_checkpoint_timestamp(&workflow.started_at)?;
        validate_checkpoint_timestamp(&workflow.ended_at)?;
        validate_checkpoint_timestamp(&workflow.original_deadline_at)?;
        let ended_at = time::OffsetDateTime::parse(
            &workflow.ended_at,
            &time::format_description::well_known::Rfc3339,
        )
        .map_err(|_| StorageError::CheckpointStateInvalid)?;
        let deadline = ended_at
            .checked_add(workflow.policy.deadline)
            .ok_or(StorageError::CheckpointStateInvalid)?;
        let stored_deadline = time::OffsetDateTime::parse(
            &workflow.original_deadline_at,
            &time::format_description::well_known::Rfc3339,
        )
        .map_err(|_| StorageError::CheckpointStateInvalid)?;
        if deadline != stored_deadline {
            return Err(StorageError::CheckpointStateInvalid);
        }
        let started_at = time::OffsetDateTime::parse(
            &workflow.started_at,
            &time::format_description::well_known::Rfc3339,
        )
        .map_err(|_| StorageError::CheckpointStateInvalid)?;
        if started_at > ended_at {
            return Err(StorageError::CheckpointStateInvalid);
        }
        if let Some(timestamp) = &workflow.last_evaluation_at {
            validate_checkpoint_timestamp(timestamp)?;
            let evaluated_at = time::OffsetDateTime::parse(
                timestamp,
                &time::format_description::well_known::Rfc3339,
            )
            .map_err(|_| StorageError::CheckpointStateInvalid)?;
            if evaluated_at < ended_at {
                return Err(StorageError::CheckpointStateInvalid);
            }
        }
        match workflow.phase {
            QualifiedSamplingPhase::Association
                if !workflow.accepted_sample_digests.is_empty()
                    || !workflow.accepted_events.is_empty()
                    || !workflow.baseline_samples.is_empty()
                    || workflow.reconciliation_initialized
                    || workflow.last_evaluation_at.is_some() =>
            {
                return Err(StorageError::CheckpointStateInvalid)
            }
            QualifiedSamplingPhase::BeforeTask
                if workflow.reconciliation_initialized
                    || workflow.last_evaluation_at.is_some()
                    || workflow.accepted_events.iter().any(|event| {
                        event.phase
                            == crate::telemetry::task_quota_sampling::SamplingPhase::AfterTask
                    }) =>
            {
                return Err(StorageError::CheckpointStateInvalid)
            }
            QualifiedSamplingPhase::Reconciling | QualifiedSamplingPhase::Terminal
                if !workflow.reconciliation_initialized
                    || workflow.last_evaluation_at.is_none() =>
            {
                return Err(StorageError::CheckpointStateInvalid)
            }
            _ => {}
        }
        if workflow.reconciliation_initialized {
            let Some(active) = reconciliations.get(&workflow.task_id) else {
                return Err(StorageError::CheckpointStateInvalid);
            };
            if active.policy != workflow.policy
                || active.reconciliation.task.task_id != workflow.task_id
                || active.reconciliation.task.session_id != workflow.session_id
                || active.reconciliation.task.started_at.as_deref() != Some(&workflow.started_at)
                || active.reconciliation.task.ended_at.as_deref() != Some(&workflow.ended_at)
            {
                return Err(StorageError::CheckpointStateInvalid);
            }
            if active.reconciliation.task.known_overlapping_task_ids
                != workflow.known_overlapping_task_ids
            {
                return Err(StorageError::CheckpointStateInvalid);
            }
            let at = workflow
                .last_evaluation_at
                .as_deref()
                .ok_or(StorageError::CheckpointStateInvalid)?;
            let inputs: Vec<_> = workflow
                .accepted_events
                .iter()
                .filter(|event| {
                    event.phase == crate::telemetry::task_quota_sampling::SamplingPhase::AfterTask
                })
                .map(|event| event.attempt.clone())
                .collect();
            let batch =
                reconcile_task_quota_at(&active.reconciliation, &inputs, &workflow.policy, at)
                    .map_err(|_| StorageError::CheckpointStateInvalid)?;
            let terminal = batch
                .actions
                .contains(&crate::telemetry::ReconciliationAction::ReconciliationComplete);
            if terminal != (workflow.phase == QualifiedSamplingPhase::Terminal) {
                return Err(StorageError::CheckpointStateInvalid);
            }
        }
        if !workflow.baseline_samples.is_empty() {
            let target = TaskReconciliationTarget {
                task_id: workflow.task_id.clone(),
                session_id: workflow.session_id.clone(),
                started_at: Some(workflow.started_at.clone()),
                ended_at: Some(workflow.ended_at.clone()),
                known_overlapping_task_ids: workflow.known_overlapping_task_ids.clone(),
            };
            begin_task_quota_reconciliation(target, &workflow.baseline_samples, &workflow.policy)
                .map_err(|_| StorageError::CheckpointStateInvalid)?;
        }
        if !workflow.accepted_events.is_empty() || !workflow.baseline_samples.is_empty() {
            let mut digests = BTreeMap::new();
            let mut identities = BTreeSet::new();
            for sample in &workflow.baseline_samples {
                if sample.sample_id.is_empty()
                    || sample.sample_id.len() > MAX_CHECKPOINT_IDENTIFIER_BYTES
                    || sample.sampled_at.len() > 64
                {
                    return Err(StorageError::CheckpointStateInvalid);
                }
                let bytes =
                    serde_json::to_vec(sample).map_err(|_| StorageError::CheckpointStateInvalid)?;
                if bytes.len() > 16 * 1024 {
                    return Err(StorageError::CheckpointStateInvalid);
                }
                let digest = Sha256::digest(bytes)
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>();
                let key = format!("{}:sample:{}", sample.meter_type.as_str(), sample.sample_id);
                if !identities.insert(key.clone()) {
                    return Err(StorageError::CheckpointStateInvalid);
                }
                digests.insert(key, digest);
            }
            for event in &workflow.accepted_events {
                if event.phase != crate::telemetry::task_quota_sampling::SamplingPhase::AfterTask {
                    return Err(StorageError::CheckpointStateInvalid);
                }
                let id = match &event.attempt.result {
                    QuotaAcquisitionResult::Sample(sample) => {
                        if sample.sample_id.is_empty()
                            || sample.sample_id.len() > MAX_CHECKPOINT_IDENTIFIER_BYTES
                            || sample.meter_type != event.attempt.meter_type
                            || event.attempt.sampled_at != sample.sampled_at
                        {
                            return Err(StorageError::CheckpointStateInvalid);
                        }
                        let bytes = serde_json::to_vec(sample)
                            .map_err(|_| StorageError::CheckpointStateInvalid)?;
                        if bytes.len() > 16 * 1024 || sample.sampled_at.len() > 64 {
                            return Err(StorageError::CheckpointStateInvalid);
                        }
                        let digest = Sha256::digest(bytes)
                            .iter()
                            .map(|b| format!("{b:02x}"))
                            .collect::<String>();
                        let key = format!(
                            "{}:sample:{}",
                            event.attempt.meter_type.as_str(),
                            sample.sample_id
                        );
                        digests.insert(key.clone(), digest);
                        key
                    }
                    QuotaAcquisitionResult::Failed { reason } => {
                        format!(
                            "{}:failure:{}:{reason:?}",
                            event.attempt.meter_type.as_str(),
                            event.attempt.sampled_at
                        )
                    }
                };
                if id.len() > 2 * MAX_CHECKPOINT_IDENTIFIER_BYTES || !identities.insert(id) {
                    return Err(StorageError::CheckpointStateInvalid);
                }
            }
            if digests != workflow.accepted_sample_digests {
                return Err(StorageError::CheckpointStateInvalid);
            }
        } else if !workflow.accepted_sample_digests.is_empty()
            && workflow.baseline_samples.len() > workflow.accepted_sample_digests.len()
        {
            return Err(StorageError::CheckpointStateInvalid);
        }
        for (identity, digest) in &workflow.accepted_sample_digests {
            if identity.is_empty()
                || identity.len() > 2 * MAX_CHECKPOINT_IDENTIFIER_BYTES
                || !(identity.starts_with("five_hour:") || identity.starts_with("weekly:"))
                || digest.len() != 64
                || !digest
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            {
                return Err(StorageError::CheckpointStateInvalid);
            }
        }
    }
    Ok(())
}

fn validate_source_binding(
    state: &RuntimeRecoveryState,
    source: &SourceIdentity,
) -> Result<(), StorageError> {
    if state
        .qualified_sampling
        .values()
        .any(|workflow| workflow.source_generation_id != source.source_generation())
    {
        return Err(StorageError::CheckpointSourceMismatch);
    }
    Ok(())
}

fn validate_checkpoint_timestamp(value: &str) -> Result<(), StorageError> {
    use time::{format_description::well_known::Rfc3339, OffsetDateTime};
    if value.len() > 64 || OffsetDateTime::parse(value, &Rfc3339).is_err() {
        return Err(StorageError::CheckpointStateInvalid);
    }
    Ok(())
}

fn decode_state(json: &str, checksum: &str) -> Result<RuntimeRecoveryState, StorageError> {
    if json.len() > MAX_RUNTIME_STATE_BYTES {
        return Err(StorageError::CheckpointStateInvalid);
    }
    let calculated = Sha256::digest(json.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    if calculated != checksum {
        return Err(StorageError::CheckpointCorrupt);
    }
    let dto: PersistedRuntimeState =
        serde_json::from_str(json).map_err(|_| StorageError::CheckpointDecode)?;
    dto.try_into()
}

fn decode_state_with_format(
    json: &str,
    checksum: &str,
    column_format: u32,
) -> Result<RuntimeRecoveryState, StorageError> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|_| StorageError::CheckpointDecode)?;
    let embedded = value
        .get("state_format_version")
        .and_then(serde_json::Value::as_u64)
        .and_then(|version| u32::try_from(version).ok())
        .ok_or(StorageError::CheckpointDecode)?;
    if embedded != column_format {
        return Err(StorageError::CheckpointStateInvalid);
    }
    decode_state(json, checksum)
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
    let state = decode_state_with_format(&json, &checksum, format)?;
    validate_source_binding(&state, requested_source)?;
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
    validate_source_binding(state, source)?;
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

pub(crate) fn save_in_transaction(
    transaction: &Transaction<'_>,
    source: &SourceIdentity,
    cursor: &RolloutCursor,
    state: &RuntimeRecoveryState,
    state_json: String,
    state_sha256: String,
    expected_revision: Option<u64>,
) -> Result<RuntimeCheckpoint, StorageError> {
    if source.rollout_id() != cursor.rollout_id()
        || source.source_generation() != cursor.source_generation()
    {
        return Err(StorageError::CheckpointSourceMismatch);
    }
    validate_source_binding(state, source)?;
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

pub(crate) fn save_in_transaction_idempotent(
    transaction: &Transaction<'_>,
    source: &SourceIdentity,
    cursor: &RolloutCursor,
    state: &RuntimeRecoveryState,
    state_json: String,
    state_sha256: String,
    expected_revision: Option<u64>,
) -> Result<RuntimeCheckpoint, StorageError> {
    if source.rollout_id() != cursor.rollout_id()
        || source.source_generation() != cursor.source_generation()
    {
        return Err(StorageError::CheckpointSourceMismatch);
    }
    let existing = transaction
        .query_row(
            "SELECT rollout_id, source_generation, committed_offset, last_ordinal, checkpoint_revision, state_format_version, state_json, state_sha256 FROM runtime_checkpoints WHERE rollout_id = ?1 AND source_generation = ?2",
            params![source.rollout_id(), source.source_generation()],
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
    if let Some((
        rollout_id,
        source_generation,
        offset,
        ordinal,
        revision,
        format,
        existing_json,
        existing_sha256,
    )) = existing
    {
        let existing_source = SourceIdentity::new(rollout_id, source_generation)
            .map_err(|_| StorageError::CheckpointCorrupt)?;
        let committed_offset = parse_u64(&offset)?;
        let last_ordinal = ordinal.as_deref().map(parse_u64).transpose()?;
        let revision = u64::try_from(revision).map_err(|_| StorageError::CheckpointCorrupt)?;
        if revision == 0 {
            return Err(StorageError::CheckpointCorrupt);
        }
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
        let existing_state = decode_state_with_format(&existing_json, &existing_sha256, format)?;
        let existing_cursor =
            RolloutCursor::from_checkpoint(&existing_source, committed_offset, last_ordinal);
        if existing_source == *source
            && existing_cursor == *cursor
            && format == STATE_FORMAT_VERSION
            && existing_json == state_json
            && existing_sha256 == state_sha256
        {
            return Ok(RuntimeCheckpoint {
                source: existing_source,
                cursor: existing_cursor,
                state: existing_state,
                revision,
            });
        }
    }
    save_in_transaction(
        transaction,
        source,
        cursor,
        state,
        state_json,
        state_sha256,
        expected_revision,
    )
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
    fn format_v3_round_trips_non_authoritative_sampling_metadata() {
        let path = std::env::temp_dir().join(format!(
            "codex-meter-qualified-checkpoint-{}.sqlite",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let source = source("generation-1");
        let policy = ReconciliationPolicy {
            max_before_sample_age: Duration::minutes(30),
            post_task_sample_offsets: vec![Duration::seconds(10), Duration::seconds(20)],
            stabilization_not_before: Duration::seconds(10),
            required_stable_confirmations: 2,
            deadline: Duration::seconds(30),
        };
        let workflow = QualifiedSamplingCheckpoint {
            source_generation_id: "generation-1".to_owned(),
            task_id: "task-qualified-1".to_owned(),
            session_id: Some("session-qualified-1".to_owned()),
            started_at: "2026-10-04T10:00:00Z".to_owned(),
            ended_at: "2026-10-04T10:01:00Z".to_owned(),
            contract_version: 1,
            authority_contract: "synthetic-task-quota/v1".to_owned(),
            phase: QualifiedSamplingPhase::BeforeTask,
            policy,
            original_deadline_at: "2026-10-04T10:01:30Z".to_owned(),
            last_evaluation_at: None,
            accepted_sample_digests: [("five_hour:sample-a".to_owned(), "a".repeat(64))]
                .into_iter()
                .collect(),
            reconciliation_initialized: false,
            baseline_samples: Vec::new(),
            accepted_events: Vec::new(),
            known_overlapping_task_ids: Default::default(),
        };
        let workflow_id = workflow.workflow_id();
        let state = RuntimeRecoveryState {
            qualified_sampling: [(workflow_id.clone(), workflow.clone())]
                .into_iter()
                .collect(),
            ..RuntimeRecoveryState::default()
        };
        {
            let mut store = SqliteStore::open(&path).unwrap();
            store
                .save_runtime_checkpoint(&source, &RolloutCursor::at_start(&source), &state, None)
                .unwrap();
        }
        // Reopening a file-backed database exercises process-independent recovery.
        let store = SqliteStore::open(&path).unwrap();
        let recovered = store.load_runtime_checkpoint(&source).unwrap().unwrap();
        assert_eq!(recovered.state.qualified_sampling[&workflow_id], workflow);

        // Persisted authority labels remain descriptive metadata and are not
        // converted into an AssociationCredential during checkpoint recovery.
        assert_eq!(
            recovered.state.qualified_sampling[&workflow_id].authority_contract,
            "synthetic-task-quota/v1"
        );
        drop(store);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
        let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
    }

    #[test]
    fn file_backed_sampling_state_resumes_from_registered_to_initialized() {
        use crate::telemetry::task_quota_sampling::tests::synthetic;
        use crate::telemetry::task_quota_sampling::{
            SamplingPhase, SamplingSnapshot, TaskQuotaSampling,
        };

        let path = std::env::temp_dir().join(format!(
            "codex-meter-sampling-resume-{}.sqlite",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let credential = synthetic::credential();
        let source = source(&synthetic::source_generation(&credential));
        let mut coordinator = TaskQuotaSampling::default();
        coordinator
            .register(&synthetic::claim(&credential), Some(&credential))
            .unwrap();
        let registered = coordinator.snapshot().unwrap();
        {
            let mut store = SqliteStore::open(&path).unwrap();
            let committed = store
                .save_sampling_snapshot(&source, &registered, None)
                .unwrap();
            // Simulate a process restart after commit but before acknowledgment.
            let replay = store
                .save_sampling_snapshot(&source, &registered, None)
                .unwrap();
            assert_eq!(replay.revision, committed.revision);
        }

        let loaded = {
            let store = SqliteStore::open(&path).unwrap();
            store.load_runtime_checkpoint(&source).unwrap().unwrap()
        };
        assert_eq!(loaded.cursor.committed_offset(), 0);
        let checkpoint = loaded
            .state
            .qualified_sampling
            .values()
            .next()
            .unwrap()
            .clone();
        let mut coordinator = TaskQuotaSampling::recover(SamplingSnapshot {
            checkpoint,
            reconciliation: None,
        })
        .unwrap()
        .reauthorize(&credential)
        .unwrap();
        let before = [
            synthetic::sample(
                &credential,
                SamplingPhase::BeforeTask,
                QuotaMeterType::FiveHour,
                "baseline-5h",
                "2026-10-04T09:59:00Z",
                20.0,
            ),
            synthetic::sample(
                &credential,
                SamplingPhase::BeforeTask,
                QuotaMeterType::Weekly,
                "baseline-week",
                "2026-10-04T09:59:00Z",
                30.0,
            ),
        ];
        coordinator.admit_before(&before).unwrap();
        let baseline_snapshot = coordinator.snapshot().unwrap();
        {
            let mut store = SqliteStore::open(&path).unwrap();
            store
                .save_sampling_snapshot(&source, &baseline_snapshot, Some(loaded.revision))
                .unwrap();
        }

        let loaded = {
            let store = SqliteStore::open(&path).unwrap();
            store.load_runtime_checkpoint(&source).unwrap().unwrap()
        };
        let checkpoint = loaded
            .state
            .qualified_sampling
            .values()
            .next()
            .unwrap()
            .clone();
        let mut coordinator = TaskQuotaSampling::recover(SamplingSnapshot {
            checkpoint,
            reconciliation: None,
        })
        .unwrap()
        .reauthorize(&credential)
        .unwrap();
        assert_eq!(
            coordinator
                .snapshot()
                .unwrap()
                .checkpoint
                .baseline_samples
                .len(),
            2
        );
        coordinator.initialize("2026-10-04T10:05:00Z").unwrap();
        let initialized = coordinator.snapshot().unwrap();
        {
            let mut store = SqliteStore::open(&path).unwrap();
            store
                .save_sampling_snapshot(&source, &initialized, Some(loaded.revision))
                .unwrap();
        }

        let mut store = SqliteStore::open(&path).unwrap();
        let loaded = store.load_runtime_checkpoint(&source).unwrap().unwrap();
        assert_eq!(loaded.revision, 3);
        assert_eq!(loaded.cursor.committed_offset(), 0);
        let checkpoint = loaded
            .state
            .qualified_sampling
            .values()
            .next()
            .unwrap()
            .clone();
        let reconciliation = loaded
            .state
            .active_reconciliations
            .get(&checkpoint.task_id)
            .unwrap()
            .clone();
        let mut restored = TaskQuotaSampling::recover(SamplingSnapshot {
            checkpoint,
            reconciliation: Some(reconciliation),
        })
        .unwrap()
        .reauthorize(&credential)
        .unwrap();
        assert_eq!(
            restored.batch().unwrap().next_state,
            coordinator.batch().unwrap().next_state
        );
        let post = synthetic::sample(
            &credential,
            SamplingPhase::AfterTask,
            QuotaMeterType::FiveHour,
            "post-restart-sample",
            "2026-10-04T10:05:15Z",
            25.0,
        );
        assert_eq!(
            restored
                .advance(std::slice::from_ref(&post), "2026-10-04T10:05:15Z")
                .unwrap(),
            vec![crate::telemetry::task_quota_sampling::Admission::Accepted]
        );
        let post_snapshot = restored.snapshot().unwrap();
        let post_checkpoint = store
            .save_sampling_snapshot(&source, &post_snapshot, Some(loaded.revision))
            .unwrap();
        drop(store);

        let store = SqliteStore::open(&path).unwrap();
        let loaded = store.load_runtime_checkpoint(&source).unwrap().unwrap();
        assert_eq!(loaded.revision, post_checkpoint.revision);
        let workflow = loaded
            .state
            .qualified_sampling
            .values()
            .next()
            .unwrap()
            .clone();
        let reconciliation = loaded
            .state
            .active_reconciliations
            .get(&workflow.task_id)
            .unwrap()
            .clone();
        let mut restored = TaskQuotaSampling::recover(SamplingSnapshot {
            checkpoint: workflow,
            reconciliation: Some(reconciliation),
        })
        .unwrap()
        .reauthorize(&credential)
        .unwrap();
        assert_eq!(restored.snapshot().unwrap(), post_snapshot);
        assert_eq!(
            restored.advance(&[post], "2026-10-04T10:05:30Z").unwrap(),
            vec![crate::telemetry::task_quota_sampling::Admission::Replay]
        );
        let state = &restored.batch().unwrap().next_state.five_hour.state;
        assert!(
            matches!(state, crate::telemetry::MeterReconciliationState::Reconciling { candidate: Some(candidate) } if candidate.consecutive_confirmations == 1)
        );
        drop(store);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
        let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
    }

    #[test]
    fn qualified_sampling_metadata_is_bound_to_source_generation() {
        let mut store = SqliteStore::open_in_memory().unwrap();
        let wrong_source = source("generation-2");
        let workflow = QualifiedSamplingCheckpoint {
            source_generation_id: "generation-1".to_owned(),
            task_id: "task-1".to_owned(),
            session_id: None,
            started_at: "2026-10-04T10:00:00Z".to_owned(),
            ended_at: "2026-10-04T10:01:00Z".to_owned(),
            contract_version: 1,
            authority_contract: "unknown/v1".to_owned(),
            phase: QualifiedSamplingPhase::Association,
            policy: ReconciliationPolicy {
                max_before_sample_age: Duration::minutes(30),
                post_task_sample_offsets: vec![Duration::seconds(10)],
                stabilization_not_before: Duration::seconds(10),
                required_stable_confirmations: 1,
                deadline: Duration::seconds(10),
            },
            original_deadline_at: "2026-10-04T10:01:10Z".to_owned(),
            last_evaluation_at: None,
            accepted_sample_digests: BTreeMap::new(),
            reconciliation_initialized: false,
            baseline_samples: Vec::new(),
            accepted_events: Vec::new(),
            known_overlapping_task_ids: Default::default(),
        };
        let state = RuntimeRecoveryState {
            qualified_sampling: [(workflow.workflow_id(), workflow)].into_iter().collect(),
            ..RuntimeRecoveryState::default()
        };
        assert!(matches!(
            store.save_runtime_checkpoint(
                &wrong_source,
                &RolloutCursor::at_start(&wrong_source),
                &state,
                None
            ),
            Err(StorageError::CheckpointSourceMismatch)
        ));
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
    fn checkpoint_row_and_payload_versions_must_match() {
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
                "UPDATE runtime_checkpoints SET state_format_version = 2",
                [],
            )
            .unwrap();
        assert!(matches!(
            store.load_runtime_checkpoint(&source),
            Err(StorageError::CheckpointStateInvalid)
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
        let (json, _checksum): (String, String) = store
            .connection()
            .query_row(
                "SELECT state_json, state_sha256 FROM runtime_checkpoints",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        let mut state_json: serde_json::Value = serde_json::from_str(&json).unwrap();
        state_json["state_format_version"] = serde_json::json!(1);
        let json = serde_json::to_string(&state_json).unwrap();
        let checksum = Sha256::digest(json.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
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
    fn default_v3_state_checksum_matches_canonical_payload() {
        let (json, checksum) = encode_state(&RuntimeRecoveryState::default()).unwrap();
        let digest = Sha256::digest(json.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        assert_eq!(checksum, digest);
        assert!(json.contains("\"state_format_version\":3"));
        assert!(json.contains("\"qualified_sampling\":{}"));
    }

    #[test]
    fn checkpoint_v1_task_state_defaults_new_lifecycle_timestamps() {
        let mut state = RuntimeRecoveryState::default();
        state.telemetry_state.tasks.insert(
            "task-1".to_owned(),
            TaskState {
                task_id: "task-1".to_owned(),
                root_task_id: None,
                observed_root_task_ids: BTreeSet::new(),
                lifecycle: TaskLifecycle::Active,
                start_observed: true,
                started_at: Some("2025-01-01T00:00:00Z".to_owned()),
                terminal_at: None,
                terminal_evidence: BTreeSet::new(),
                anomalies: BTreeSet::new(),
                configuration_override: ConfigurationState::default(),
                token_configuration_fingerprints: BTreeSet::new(),
                configuration_consistency: ConfigurationConsistency::NoTokenConsumption,
            },
        );
        let (encoded, _) = encode_state(&state).unwrap();
        let mut old: serde_json::Value = serde_json::from_str(&encoded).unwrap();
        old["state_format_version"] = serde_json::json!(1);
        let task = &mut old["telemetry_state"]["tasks"]["task-1"];
        task.as_object_mut().unwrap().remove("started_at");
        task.as_object_mut().unwrap().remove("terminal_at");
        let old_bytes = serde_json::to_vec(&old).unwrap();
        let old_json = String::from_utf8(old_bytes).unwrap();
        let checksum = Sha256::digest(old_json.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();

        let recovered = decode_state(&old_json, &checksum).unwrap();
        let task = recovered.telemetry_state.tasks.get("task-1").unwrap();
        assert_eq!(task.started_at, None);
        assert_eq!(task.terminal_at, None);
    }

    #[test]
    fn checkpoint_v2_without_sampling_metadata_remains_compatible() {
        let (encoded, _) = encode_state(&RuntimeRecoveryState::default()).unwrap();
        let mut old: serde_json::Value = serde_json::from_str(&encoded).unwrap();
        old["state_format_version"] = serde_json::json!(2);
        old.as_object_mut().unwrap().remove("qualified_sampling");
        let old_json = serde_json::to_string(&old).unwrap();
        let checksum = Sha256::digest(old_json.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let recovered = decode_state_with_format(&old_json, &checksum, 2).unwrap();
        assert!(recovered.qualified_sampling.is_empty());
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
                started_at: Some("2025-01-01T00:00:00Z".to_owned()),
                terminal_at: None,
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
