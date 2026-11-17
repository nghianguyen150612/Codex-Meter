//! In-memory admission of task-associated quota evidence; P012 owns reconciliation.
//!
//! No production authority is supported. Claims and account snapshots are not
//! credentials. The only issuer lives in this module's `cfg(test)` fixtures.
//! Nothing here proves account-wide isolation or exclusive task consumption.

use std::collections::{BTreeMap, BTreeSet};

use time::{format_description::well_known::Rfc3339, OffsetDateTime};

use super::normalized::{NormalizedQuotaSample, QuotaMeterType, QuotaWindowIdentity};
use super::quota_reconciliation::{
    begin_task_quota_reconciliation, reconcile_task_quota_at, MeterReconciliationState,
    QuotaAcquisitionAttempt, QuotaAcquisitionResult, ReconciliationAction, ReconciliationBatch,
    ReconciliationError, ReconciliationPolicy, TaskQuotaReconciliation, TaskReconciliationTarget,
};
use crate::storage::{
    ActiveReconciliationCheckpoint, QualifiedSamplingCheckpoint, QualifiedSamplingPhase,
};

/// Untrusted association data. Possession of this value grants no authority.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssociationClaim {
    pub contract_version: u32,
    pub task: TaskReconciliationTarget,
    pub source_generation_id: String,
    /// The complete validated policy is its identity, avoiding caller-chosen aliases.
    pub policy: ReconciliationPolicy,
    pub phase: SamplingPhase,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SamplingPhase {
    TaskAssociation,
    BeforeTask,
    AfterTask,
}

/// Opaque, non-deserializable authority capability bound to the entire claim.
/// There is deliberately no production constructor or conversion from raw data.
///
/// ```compile_fail
/// use codex_meter::telemetry::task_quota_sampling::AssociationCredential;
/// let credential = AssociationCredential {
///     authority_contract: "synthetic-task-quota/v1",
///     binding: todo!(),
/// };
/// ```
///
/// ```compile_fail
/// use codex_meter::telemetry::task_quota_sampling::AssociationCredential;
/// let credential: AssociationCredential = serde_json::from_str("{}").unwrap();
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssociationCredential {
    authority_contract: &'static str,
    binding: AssociationClaim,
}

/// Opaque event: provenance and payload cannot be edited by rollout adapters.
/// In P044B1 only the private synthetic unit-test provider can issue these.
///
/// ```compile_fail
/// use codex_meter::telemetry::task_quota_sampling::QualifiedQuotaEvent;
/// let event: QualifiedQuotaEvent = serde_json::from_str("{}").unwrap();
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct QualifiedQuotaEvent {
    credential: AssociationCredential,
    phase: SamplingPhase,
    attempt: QuotaAcquisitionAttempt,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Admission {
    Accepted,
    Replay,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SamplingRejection {
    MissingAuthority,
    UnsupportedAuthority,
    UnsupportedContractVersion,
    WrongTask,
    WrongSession,
    WrongSourceGeneration,
    WrongPolicy,
    WrongPhase,
    ConflictingAssociation,
    InvalidIdentity,
    NotRegistered,
    AlreadyInitialized,
    NotInitialized,
    MeterMismatch,
    ConflictingDuplicate,
    TerminalMeter,
    EvaluationTimeReversed,
    Reconciliation(ReconciliationError),
    InvalidSnapshot,
}

impl From<ReconciliationError> for SamplingRejection {
    fn from(value: ReconciliationError) -> Self {
        Self::Reconciliation(value)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SamplingStage {
    Uninitialized,
    QualifiedAssociation,
    Reconciling,
}

#[derive(Clone, Debug)]
enum State {
    Uninitialized,
    Qualified {
        association: Box<AssociationCredential>,
        before: Vec<NormalizedQuotaSample>,
        evidence: Evidence,
    },
    Active(Box<Active>),
}

type Evidence = BTreeMap<(String, String), QualifiedQuotaEvent>;

#[derive(Clone, Debug)]
struct Active {
    association: AssociationCredential,
    evidence: Evidence,
    batch: ReconciliationBatch,
    evaluated_at: OffsetDateTime,
}

/// One immutable task association and independent P012 meter results.
#[derive(Clone, Debug)]
pub struct TaskQuotaSampling {
    state: State,
}

impl Default for TaskQuotaSampling {
    fn default() -> Self {
        Self {
            state: State::Uninitialized,
        }
    }
}

impl TaskQuotaSampling {
    /// Apply a coordinator transition and commit its snapshot with revision CAS.
    /// The live coordinator changes only after the SQLite transaction succeeds.
    pub fn transition_and_persist<T>(
        &mut self,
        store: &mut crate::storage::SqliteStore,
        source: &super::SourceIdentity,
        expected_revision: Option<u64>,
        transition: impl FnOnce(&mut Self) -> Result<T, SamplingRejection>,
    ) -> Result<(T, crate::storage::RuntimeCheckpoint), DurableSamplingError> {
        let mut next = self.clone();
        let result = transition(&mut next).map_err(DurableSamplingError::Sampling)?;
        let snapshot = next.snapshot().map_err(DurableSamplingError::Sampling)?;
        let checkpoint = store
            .save_sampling_snapshot(source, &snapshot, expected_revision)
            .map_err(DurableSamplingError::Storage)?;
        *self = next;
        Ok((result, checkpoint))
    }

    /// Export normalized, bounded recovery material. This snapshot is data only;
    /// it never contains the opaque authority credential.
    pub fn snapshot(&self) -> Result<SamplingSnapshot, SamplingRejection> {
        let (association, before, evidence, phase, batch, evaluated_at) = match &self.state {
            State::Uninitialized => return Err(SamplingRejection::NotRegistered),
            State::Qualified {
                association,
                before,
                evidence,
            } => (
                association.as_ref(),
                before.as_slice(),
                evidence,
                if before.is_empty() && evidence.is_empty() {
                    QualifiedSamplingPhase::Association
                } else {
                    QualifiedSamplingPhase::BeforeTask
                },
                None,
                None,
            ),
            State::Active(active) => (
                &active.association,
                &[][..],
                &active.evidence,
                if self.is_complete() {
                    QualifiedSamplingPhase::Terminal
                } else {
                    QualifiedSamplingPhase::Reconciling
                },
                Some(active.batch.next_state.clone()),
                Some(active.evaluated_at),
            ),
        };
        let binding = &association.binding;
        let task = &binding.task;
        let (started_at, ended_at) = match (&task.started_at, &task.ended_at) {
            (Some(start), Some(end)) => (start.clone(), end.clone()),
            _ => return Err(SamplingRejection::InvalidIdentity),
        };
        let deadline = evaluation_time(&ended_at)?
            .checked_add(binding.policy.deadline)
            .ok_or(SamplingRejection::InvalidSnapshot)?;
        let mut accepted_sample_digests = BTreeMap::new();
        for ((meter, identity), event) in evidence {
            let QuotaAcquisitionResult::Sample(sample) = &event.attempt.result else {
                continue;
            };
            accepted_sample_digests.insert(format!("{meter}:{identity}"), sample_digest(sample)?);
        }
        let baseline_samples = evidence
            .values()
            .filter(|event| event.phase == SamplingPhase::BeforeTask)
            .filter_map(|event| match &event.attempt.result {
                QuotaAcquisitionResult::Sample(sample) => Some(sample.clone()),
                QuotaAcquisitionResult::Failed { .. } => None,
            })
            .collect();
        let events = evidence
            .values()
            .filter(|event| event.phase == SamplingPhase::AfterTask)
            .map(|event| StoredSamplingEvent {
                phase: event.phase,
                attempt: event.attempt.clone(),
            })
            .collect();
        let checkpoint = QualifiedSamplingCheckpoint {
            source_generation_id: binding.source_generation_id.clone(),
            task_id: task.task_id.clone(),
            session_id: task.session_id.clone(),
            started_at,
            ended_at,
            contract_version: binding.contract_version,
            authority_contract: association.authority_contract.to_owned(),
            phase,
            policy: binding.policy.clone(),
            original_deadline_at: deadline
                .format(&Rfc3339)
                .map_err(|_| SamplingRejection::InvalidSnapshot)?,
            last_evaluation_at: evaluated_at
                .map(|value| value.format(&Rfc3339))
                .transpose()
                .map_err(|_| SamplingRejection::InvalidSnapshot)?,
            accepted_sample_digests,
            reconciliation_initialized: batch.is_some(),
            baseline_samples: if batch.is_some() {
                baseline_samples
            } else {
                before.to_vec()
            },
            accepted_events: events,
            known_overlapping_task_ids: task.known_overlapping_task_ids.clone(),
        };
        if checkpoint.accepted_events.len() > 512 {
            return Err(SamplingRejection::InvalidSnapshot);
        }
        let reconciliation = batch.map(|reconciliation| ActiveReconciliationCheckpoint {
            policy: binding.policy.clone(),
            reconciliation,
        });
        let snapshot = SamplingSnapshot {
            checkpoint,
            reconciliation,
        };
        validate_snapshot(&snapshot)?;
        Ok(snapshot)
    }

    /// Decode validated checkpoint data into an inspectable but unauthorized value.
    pub fn recover(
        snapshot: SamplingSnapshot,
    ) -> Result<RecoveredTaskQuotaSampling, SamplingRejection> {
        validate_snapshot(&snapshot)?;
        Ok(RecoveredTaskQuotaSampling { snapshot })
    }
    /// Register a claim only with a matching sealed capability. Raw rollout input
    /// must enter without a credential and cannot promote itself to trusted data.
    pub fn register(
        &mut self,
        claim: &AssociationClaim,
        credential: Option<&AssociationCredential>,
    ) -> Result<Admission, SamplingRejection> {
        let credential = credential.ok_or(SamplingRejection::MissingAuthority)?;
        validate_credential(credential)?;
        validate_binding(claim, &credential.binding)?;
        match &self.state {
            State::Uninitialized => {
                self.state = State::Qualified {
                    association: Box::new(credential.clone()),
                    before: Vec::new(),
                    evidence: BTreeMap::new(),
                };
                Ok(Admission::Accepted)
            }
            State::Qualified { association, .. } if association.as_ref() == credential => {
                Ok(Admission::Replay)
            }
            State::Active(active) if active.association == *credential => Ok(Admission::Replay),
            _ => Err(SamplingRejection::ConflictingAssociation),
        }
    }

    /// Validate the whole batch before admitting any baseline evidence. P012
    /// subsequently selects eligible baselines, including missing/stale behavior.
    pub fn admit_before(
        &mut self,
        events: &[QualifiedQuotaEvent],
    ) -> Result<Vec<Admission>, SamplingRejection> {
        let State::Qualified {
            association,
            before,
            evidence,
        } = &mut self.state
        else {
            return Err(match self.state {
                State::Uninitialized => SamplingRejection::NotRegistered,
                _ => SamplingRejection::AlreadyInitialized,
            });
        };
        let (next_evidence, admissions) = admit_events(
            association,
            evidence,
            events,
            SamplingPhase::BeforeTask,
            None,
        )?;
        for (event, admission) in events.iter().zip(&admissions) {
            if *admission == Admission::Accepted {
                let QuotaAcquisitionResult::Sample(sample) = &event.attempt.result else {
                    return Err(SamplingRejection::WrongPhase);
                };
                before.push(sample.clone());
            }
        }
        *evidence = next_evidence;
        Ok(admissions)
    }

    pub fn initialize(
        &mut self,
        evaluate_at: &str,
    ) -> Result<&ReconciliationBatch, SamplingRejection> {
        let State::Qualified {
            association,
            before,
            evidence,
        } = &self.state
        else {
            return Err(match self.state {
                State::Uninitialized => SamplingRejection::NotRegistered,
                _ => SamplingRejection::AlreadyInitialized,
            });
        };
        let binding = &association.binding;
        let initial =
            begin_task_quota_reconciliation(binding.task.clone(), before, &binding.policy)?;
        let batch = reconcile_task_quota_at(&initial, &[], &binding.policy, evaluate_at)?;
        let evaluated_at = evaluation_time(evaluate_at)?;
        self.state = State::Active(Box::new(Active {
            association: association.as_ref().clone(),
            evidence: evidence.clone(),
            batch,
            evaluated_at,
        }));
        Ok(self.batch().expect("initialized"))
    }

    /// Atomic admission and P012 evaluation. Identical replays are forwarded to
    /// P012's own deduplication, including evidence previously ahead of evaluation.
    /// Ordering within each call is exactly P012's timestamp/meter/identity order.
    pub fn advance(
        &mut self,
        events: &[QualifiedQuotaEvent],
        evaluate_at: &str,
    ) -> Result<Vec<Admission>, SamplingRejection> {
        let State::Active(active) = &mut self.state else {
            return Err(SamplingRejection::NotInitialized);
        };
        let evaluated_at = evaluation_time(evaluate_at)?;
        if evaluated_at < active.evaluated_at {
            return Err(SamplingRejection::EvaluationTimeReversed);
        }
        let (evidence, admissions) = admit_events(
            &active.association,
            &active.evidence,
            events,
            SamplingPhase::AfterTask,
            Some(&active.batch.next_state),
        )?;
        let mut eligible = Vec::new();
        for event in evidence
            .values()
            .filter(|event| event.phase == SamplingPhase::AfterTask)
        {
            if evaluation_time(&event.attempt.sampled_at)? <= evaluated_at {
                eligible.push(event.attempt.clone());
            }
        }
        let batch = reconcile_task_quota_at(
            &active.batch.next_state,
            &eligible,
            &active.association.binding.policy,
            evaluate_at,
        )?;
        active.batch = batch;
        active.evidence = evidence;
        active.evaluated_at = evaluated_at;
        Ok(admissions)
    }

    pub fn stage(&self) -> SamplingStage {
        match self.state {
            State::Uninitialized => SamplingStage::Uninitialized,
            State::Qualified { .. } => SamplingStage::QualifiedAssociation,
            State::Active(_) => SamplingStage::Reconciling,
        }
    }

    /// Read-only P012 states, outputs, and current pending actions.
    pub fn batch(&self) -> Option<&ReconciliationBatch> {
        match &self.state {
            State::Active(active) => Some(&active.batch),
            _ => None,
        }
    }

    pub fn is_complete(&self) -> bool {
        self.batch().is_some_and(|batch| {
            batch
                .actions
                .contains(&ReconciliationAction::ReconciliationComplete)
        })
    }
}

#[derive(Debug)]
pub enum DurableSamplingError {
    Sampling(SamplingRejection),
    Storage(crate::storage::StorageError),
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct StoredSamplingEvent {
    pub phase: SamplingPhase,
    pub attempt: QuotaAcquisitionAttempt,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SamplingSnapshot {
    pub checkpoint: QualifiedSamplingCheckpoint,
    pub reconciliation: Option<ActiveReconciliationCheckpoint>,
}

/// Data recovered from disk cannot sample until a recognized live credential is supplied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoveredTaskQuotaSampling {
    snapshot: SamplingSnapshot,
}

impl RecoveredTaskQuotaSampling {
    pub fn checkpoint(&self) -> &QualifiedSamplingCheckpoint {
        &self.snapshot.checkpoint
    }

    pub fn reauthorize(
        self,
        credential: &AssociationCredential,
    ) -> Result<TaskQuotaSampling, SamplingRejection> {
        validate_credential(credential)?;
        let checkpoint = &self.snapshot.checkpoint;
        let claim = &credential.binding;
        if credential.authority_contract != checkpoint.authority_contract {
            return Err(SamplingRejection::UnsupportedAuthority);
        }
        if claim.contract_version != checkpoint.contract_version {
            return Err(SamplingRejection::UnsupportedContractVersion);
        }
        if claim.source_generation_id != checkpoint.source_generation_id {
            return Err(SamplingRejection::WrongSourceGeneration);
        }
        if claim.task.task_id != checkpoint.task_id {
            return Err(SamplingRejection::WrongTask);
        }
        if claim.task.session_id != checkpoint.session_id {
            return Err(SamplingRejection::WrongSession);
        }
        if claim.task.started_at.as_deref() != Some(checkpoint.started_at.as_str())
            || claim.task.ended_at.as_deref() != Some(checkpoint.ended_at.as_str())
            || claim.task.known_overlapping_task_ids != checkpoint.known_overlapping_task_ids
        {
            return Err(SamplingRejection::ConflictingAssociation);
        }
        if claim.policy != checkpoint.policy {
            return Err(SamplingRejection::WrongPolicy);
        }
        let mut evidence = BTreeMap::new();
        for stored in &checkpoint.accepted_events {
            let event = QualifiedQuotaEvent {
                credential: credential.clone(),
                phase: stored.phase,
                attempt: stored.attempt.clone(),
            };
            let identity = match &event.attempt.result {
                QuotaAcquisitionResult::Sample(sample) => format!("sample:{}", sample.sample_id),
                QuotaAcquisitionResult::Failed { reason } => {
                    format!("failure:{}:{reason:?}", event.attempt.sampled_at)
                }
            };
            evidence.insert(
                (event.attempt.meter_type.as_str().to_owned(), identity),
                event,
            );
        }
        for sample in &checkpoint.baseline_samples {
            let event = QualifiedQuotaEvent {
                credential: credential.clone(),
                phase: SamplingPhase::BeforeTask,
                attempt: QuotaAcquisitionAttempt {
                    sampled_at: sample.sampled_at.clone(),
                    meter_type: sample.meter_type,
                    result: QuotaAcquisitionResult::Sample(sample.clone()),
                },
            };
            evidence.insert(
                (
                    event.attempt.meter_type.as_str().to_owned(),
                    format!("sample:{}", sample.sample_id),
                ),
                event,
            );
        }
        if !checkpoint.reconciliation_initialized {
            return Ok(TaskQuotaSampling {
                state: State::Qualified {
                    association: Box::new(credential.clone()),
                    before: checkpoint.baseline_samples.clone(),
                    evidence,
                },
            });
        }
        let active = self
            .snapshot
            .reconciliation
            .ok_or(SamplingRejection::InvalidSnapshot)?;
        let at = checkpoint
            .last_evaluation_at
            .as_deref()
            .ok_or(SamplingRejection::InvalidSnapshot)?;
        let batch = reconcile_task_quota_at(&active.reconciliation, &[], &checkpoint.policy, at)?;
        let evaluated_at = evaluation_time(at)?;
        Ok(TaskQuotaSampling {
            state: State::Active(Box::new(Active {
                association: credential.clone(),
                evidence,
                batch,
                evaluated_at,
            })),
        })
    }
}

fn validate_snapshot(snapshot: &SamplingSnapshot) -> Result<(), SamplingRejection> {
    let c = &snapshot.checkpoint;
    if c.contract_version != 1
        || c.accepted_events
            .len()
            .saturating_add(c.baseline_samples.len())
            > 512
        || c.accepted_sample_digests.len() > 512
        || c.workflow_id().is_empty()
        || c.task_id.is_empty()
        || c.task_id.len() > 256
        || c.source_generation_id.is_empty()
        || c.source_generation_id.len() > 256
        || c.authority_contract.is_empty()
        || c.authority_contract.len() > 256
        || c.session_id
            .as_ref()
            .is_some_and(|s| s.is_empty() || s.len() > 256)
    {
        return Err(SamplingRejection::InvalidSnapshot);
    }
    c.policy.validate()?;
    let end = evaluation_time(&c.ended_at)?;
    let start = evaluation_time(&c.started_at)?;
    if start > end
        || c.known_overlapping_task_ids.len() > 512
        || c.known_overlapping_task_ids
            .iter()
            .any(|id| id.is_empty() || id.len() > 256)
    {
        return Err(SamplingRejection::InvalidSnapshot);
    }
    let deadline = end
        .checked_add(c.policy.deadline)
        .ok_or(SamplingRejection::InvalidSnapshot)?;
    if evaluation_time(&c.original_deadline_at)? != deadline {
        return Err(SamplingRejection::InvalidSnapshot);
    }
    if c.reconciliation_initialized != snapshot.reconciliation.is_some() {
        return Err(SamplingRejection::InvalidSnapshot);
    }
    if c.reconciliation_initialized
        != matches!(
            c.phase,
            QualifiedSamplingPhase::Reconciling | QualifiedSamplingPhase::Terminal
        )
    {
        return Err(SamplingRejection::InvalidSnapshot);
    }
    match c.phase {
        QualifiedSamplingPhase::Association
            if !c.accepted_events.is_empty()
                || !c.accepted_sample_digests.is_empty()
                || !c.baseline_samples.is_empty()
                || c.last_evaluation_at.is_some() =>
        {
            return Err(SamplingRejection::InvalidSnapshot);
        }
        QualifiedSamplingPhase::BeforeTask
            if c.reconciliation_initialized || c.last_evaluation_at.is_some() =>
        {
            return Err(SamplingRejection::InvalidSnapshot);
        }
        QualifiedSamplingPhase::Reconciling | QualifiedSamplingPhase::Terminal
            if c.last_evaluation_at.is_none() =>
        {
            return Err(SamplingRejection::InvalidSnapshot);
        }
        _ => {}
    }
    if c.last_evaluation_at
        .as_deref()
        .is_some_and(|at| evaluation_time(at).is_ok_and(|value| value < end))
    {
        return Err(SamplingRejection::InvalidSnapshot);
    }
    if let Some(active) = &snapshot.reconciliation {
        active.reconciliation.validate_recovered(&c.policy)?;
        if active.policy != c.policy
            || active.reconciliation.task.task_id != c.task_id
            || active.reconciliation.task.session_id != c.session_id
            || active.reconciliation.task.started_at.as_deref() != Some(c.started_at.as_str())
            || active.reconciliation.task.ended_at.as_deref() != Some(c.ended_at.as_str())
            || active.reconciliation.task.known_overlapping_task_ids != c.known_overlapping_task_ids
        {
            return Err(SamplingRejection::InvalidSnapshot);
        }
        let at = c
            .last_evaluation_at
            .as_deref()
            .ok_or(SamplingRejection::InvalidSnapshot)?;
        let batch = reconcile_task_quota_at(&active.reconciliation, &[], &c.policy, at)?;
        let terminal = batch
            .actions
            .contains(&ReconciliationAction::ReconciliationComplete);
        if terminal != (c.phase == QualifiedSamplingPhase::Terminal) {
            return Err(SamplingRejection::InvalidSnapshot);
        }
    }
    let mut seen = BTreeSet::new();
    let mut expected_digests = BTreeMap::new();
    for sample in &c.baseline_samples {
        if sample.sample_id.is_empty()
            || sample.sample_id.len() > 256
            || sample.sampled_at.len() > 64
        {
            return Err(SamplingRejection::InvalidSnapshot);
        }
        let identity = format!("{}:sample:{}", sample.meter_type.as_str(), sample.sample_id);
        if !seen.insert(identity.clone()) {
            return Err(SamplingRejection::InvalidSnapshot);
        }
        expected_digests.insert(identity, sample_digest(sample)?);
    }
    for event in &c.accepted_events {
        if event.phase != SamplingPhase::AfterTask {
            return Err(SamplingRejection::InvalidSnapshot);
        }
        let identity = match &event.attempt.result {
            QuotaAcquisitionResult::Sample(s) => format!(
                "{}:sample:{}",
                event.attempt.meter_type.as_str(),
                s.sample_id
            ),
            QuotaAcquisitionResult::Failed { reason } => format!(
                "{}:failure:{}:{reason:?}",
                event.attempt.meter_type.as_str(),
                event.attempt.sampled_at
            ),
        };
        if !seen.insert(identity.clone()) {
            return Err(SamplingRejection::InvalidSnapshot);
        }
        if let QuotaAcquisitionResult::Sample(sample) = &event.attempt.result {
            if sample.meter_type != event.attempt.meter_type
                || sample.sample_id.is_empty()
                || sample.sample_id.len() > 256
                || sample.sampled_at != event.attempt.sampled_at
            {
                return Err(SamplingRejection::InvalidSnapshot);
            }
            expected_digests.insert(identity, sample_digest(sample)?);
        }
    }
    if expected_digests != c.accepted_sample_digests {
        return Err(SamplingRejection::InvalidSnapshot);
    }
    Ok(())
}

fn sample_digest(sample: &NormalizedQuotaSample) -> Result<String, SamplingRejection> {
    let bytes = serde_json::to_vec(sample).map_err(|_| SamplingRejection::InvalidSnapshot)?;
    if bytes.len() > 16 * 1024
        || sample.sample_id.is_empty()
        || sample.sample_id.len() > 256
        || sample.sampled_at.len() > 64
    {
        return Err(SamplingRejection::InvalidSnapshot);
    }
    use sha2::Digest;
    Ok(sha2::Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

fn evaluation_time(value: &str) -> Result<OffsetDateTime, SamplingRejection> {
    OffsetDateTime::parse(value, &Rfc3339)
        .map_err(|_| SamplingRejection::Reconciliation(ReconciliationError::InvalidTaskTimestamp))
}

fn validate_credential(credential: &AssociationCredential) -> Result<(), SamplingRejection> {
    // Recognition itself is compiled out of production, not controlled by input.
    let supported = {
        #[cfg(test)]
        {
            credential.authority_contract == "synthetic-task-quota/v1"
        }
        #[cfg(not(test))]
        {
            false
        }
    };
    if !supported {
        return Err(SamplingRejection::UnsupportedAuthority);
    }
    let binding = &credential.binding;
    if binding.contract_version != 1 {
        return Err(SamplingRejection::UnsupportedContractVersion);
    }
    if binding.phase != SamplingPhase::TaskAssociation {
        return Err(SamplingRejection::WrongPhase);
    }
    if binding.task.task_id.trim().is_empty()
        || binding
            .task
            .session_id
            .as_ref()
            .is_some_and(|id| id.trim().is_empty())
        || binding.source_generation_id.trim().is_empty()
    {
        return Err(SamplingRejection::InvalidIdentity);
    }
    binding.policy.validate()?;
    Ok(())
}

fn validate_binding(
    expected: &AssociationClaim,
    actual: &AssociationClaim,
) -> Result<(), SamplingRejection> {
    if expected.contract_version != actual.contract_version {
        return Err(SamplingRejection::UnsupportedContractVersion);
    }
    if expected.task.task_id != actual.task.task_id {
        return Err(SamplingRejection::WrongTask);
    }
    if expected.task.session_id != actual.task.session_id {
        return Err(SamplingRejection::WrongSession);
    }
    if expected.source_generation_id != actual.source_generation_id {
        return Err(SamplingRejection::WrongSourceGeneration);
    }
    if expected.policy != actual.policy {
        return Err(SamplingRejection::WrongPolicy);
    }
    if expected.phase != actual.phase {
        return Err(SamplingRejection::WrongPhase);
    }
    if expected.task != actual.task {
        return Err(SamplingRejection::ConflictingAssociation);
    }
    Ok(())
}

fn admit_events(
    association: &AssociationCredential,
    evidence: &Evidence,
    events: &[QualifiedQuotaEvent],
    phase: SamplingPhase,
    current: Option<&TaskQuotaReconciliation>,
) -> Result<(Evidence, Vec<Admission>), SamplingRejection> {
    let mut next = evidence.clone();
    let mut admissions = Vec::with_capacity(events.len());
    for event in events {
        validate_credential(&event.credential)?;
        validate_binding(&association.binding, &event.credential.binding)?;
        if association.authority_contract != event.credential.authority_contract {
            return Err(SamplingRejection::UnsupportedAuthority);
        }
        if event.phase != phase {
            return Err(SamplingRejection::WrongPhase);
        }
        let attempt = &event.attempt;
        let identity = match &attempt.result {
            QuotaAcquisitionResult::Sample(sample) => {
                if sample.meter_type != attempt.meter_type
                    || matches!(&sample.reset_evidence, QuotaWindowIdentity::Available { meter_type, .. } if *meter_type != sample.meter_type)
                {
                    return Err(SamplingRejection::MeterMismatch);
                }
                if sample.sample_id.trim().is_empty() {
                    return Err(SamplingRejection::InvalidIdentity);
                }
                format!("sample:{}", sample.sample_id)
            }
            QuotaAcquisitionResult::Failed { reason } => {
                if phase == SamplingPhase::BeforeTask {
                    return Err(SamplingRejection::WrongPhase);
                }
                // Match P012's failure identity rather than inventing retries.
                format!("failure:{}:{reason:?}", attempt.sampled_at)
            }
        };
        let key = (attempt.meter_type.as_str().to_owned(), identity);
        if let Some(previous) = next.get(&key) {
            // P011/P012 ignore previously seen IDs without comparing payloads.
            // This ledger adds conflict detection, not another movement policy.
            if previous != event {
                return Err(SamplingRejection::ConflictingDuplicate);
            }
            admissions.push(Admission::Replay);
        } else {
            if let Some(current) = current {
                let state = match attempt.meter_type {
                    QuotaMeterType::FiveHour => &current.five_hour.state,
                    QuotaMeterType::Weekly => &current.weekly.state,
                };
                if !matches!(
                    state,
                    MeterReconciliationState::AwaitingAfterSample
                        | MeterReconciliationState::Reconciling { .. }
                ) {
                    return Err(SamplingRejection::TerminalMeter);
                }
            }
            next.insert(key, event.clone());
            admissions.push(Admission::Accepted);
        }
    }
    Ok((next, admissions))
}

#[cfg(test)]
pub(crate) mod tests;
