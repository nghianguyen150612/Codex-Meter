//! In-memory admission of task-associated quota evidence; P012 owns reconciliation.
//!
//! No production authority is supported. Claims and account snapshots are not
//! credentials. The only issuer lives in this module's `cfg(test)` fixtures.
//! Nothing here proves account-wide isolation or exclusive task consumption.

use std::collections::BTreeMap;

use time::{format_description::well_known::Rfc3339, OffsetDateTime};

use super::normalized::{NormalizedQuotaSample, QuotaMeterType, QuotaWindowIdentity};
use super::quota_reconciliation::{
    begin_task_quota_reconciliation, reconcile_task_quota_at, MeterReconciliationState,
    QuotaAcquisitionAttempt, QuotaAcquisitionResult, ReconciliationAction, ReconciliationBatch,
    ReconciliationError, ReconciliationPolicy, TaskQuotaReconciliation, TaskReconciliationTarget,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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

#[derive(Debug)]
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

#[derive(Debug)]
struct Active {
    association: AssociationCredential,
    evidence: Evidence,
    batch: ReconciliationBatch,
    evaluated_at: OffsetDateTime,
}

/// One immutable task association and independent P012 meter results.
#[derive(Debug)]
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
        let inputs: Vec<_> = events.iter().map(|event| event.attempt.clone()).collect();
        let batch = reconcile_task_quota_at(
            &active.batch.next_state,
            &inputs,
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
mod tests;
