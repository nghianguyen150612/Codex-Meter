//! Conservative task coverage assessment. Continuity is not closure.
//!
//! No audited production source currently provides a completeness contract.
//! Only an internal authoritative adapter can supply a closure proof; none is
//! available for production rollout sources.

use std::collections::{BTreeMap, BTreeSet};

use super::{
    AttributedTokenEvent, ConfigurationConsistency, TaskState, TokenTelemetryCompleteness,
};

pub const COVERAGE_CONTRACT_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceContinuity {
    Unknown,
    Continuous,
    Discontinuous,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskCoverageStatus {
    Unavailable,
    Partial,
    ContinuousButUnproven,
    VerifiedComplete,
    Conflicting,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum TaskCoverageReason {
    MissingIdentity,
    MissingBoundary,
    ContinuityUnknown,
    SourceDiscontinuous,
    MissingStart,
    MissingTerminal,
    LifecycleConflict,
    MixedConfiguration,
    EventIdentityMismatch,
    ConflictingDuplicate,
    ClosureUnproven,
    InvalidClosure,
}

/// All identifiers must already be privacy-safe normalized identities. The boundary
/// is a candidate until committed atomically with the observation and checkpoint.
#[derive(Debug)]
pub struct TaskCoverageEvidence<'a> {
    pub task_id: &'a str,
    pub session_id: Option<&'a str>,
    pub source_instance_id: &'a str,
    pub boundary: Option<u64>,
    pub continuity: SourceContinuity,
    pub task: Option<&'a TaskState>,
    /// Canonical per-response events only; snapshots must never be passed here.
    pub events: &'a [AttributedTokenEvent],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskCoverageAssessment {
    contract_version: u32,
    continuity: SourceContinuity,
    status: TaskCoverageStatus,
    canonical_event_count: usize,
    reasons: BTreeSet<TaskCoverageReason>,
}

// Private and non-deserializable: no production adapter currently constructs this.
// A future adapter must audit the source contract before creating this value.
struct TrustedTaskClosure {
    contract: ClosureContract,
    source_instance_id: String,
    task_id: String,
    session_id: String,
    committed_boundary: u64,
    response_ids: BTreeSet<String>,
    provenance: ClosureProvenance,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ClosureContract {
    #[cfg(test)]
    SyntheticV1,
    // No audited production contract is registered.
    #[allow(dead_code)]
    Unavailable,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ClosureProvenance {
    #[cfg(test)]
    SyntheticAuthority,
    #[allow(dead_code)]
    Unavailable,
}

impl TaskCoverageAssessment {
    pub fn status(&self) -> TaskCoverageStatus {
        self.status
    }
    pub fn reasons(&self) -> &BTreeSet<TaskCoverageReason> {
        &self.reasons
    }
    pub fn canonical_event_count(&self) -> usize {
        self.canonical_event_count
    }
    pub fn continuity(&self) -> SourceContinuity {
        self.continuity
    }
    pub fn contract_version(&self) -> u32 {
        self.contract_version
    }

    /// P013 remains authoritative for checked token arithmetic and validity.
    pub fn token_completeness(&self) -> TokenTelemetryCompleteness {
        if self.status == TaskCoverageStatus::VerifiedComplete {
            TokenTelemetryCompleteness::Complete
        } else {
            TokenTelemetryCompleteness::Incomplete
        }
    }
}

pub fn assess_task_coverage(e: &TaskCoverageEvidence<'_>) -> TaskCoverageAssessment {
    assess_with_closure(e, None)
}

fn assess_with_closure(
    e: &TaskCoverageEvidence<'_>,
    closure: Option<&TrustedTaskClosure>,
) -> TaskCoverageAssessment {
    use TaskCoverageReason as R;
    let mut reasons = BTreeSet::new();
    if e.task_id.is_empty()
        || e.session_id.is_none_or(str::is_empty)
        || e.source_instance_id.is_empty()
    {
        reasons.insert(R::MissingIdentity);
    }
    if e.boundary.is_none() {
        reasons.insert(R::MissingBoundary);
    }
    match e.continuity {
        SourceContinuity::Unknown => {
            reasons.insert(R::ContinuityUnknown);
        }
        SourceContinuity::Discontinuous => {
            reasons.insert(R::SourceDiscontinuous);
        }
        SourceContinuity::Continuous => {}
    }
    match e.task {
        None => {
            reasons.insert(R::MissingStart);
            reasons.insert(R::MissingTerminal);
        }
        Some(task) => {
            if task.task_id != e.task_id {
                reasons.insert(R::EventIdentityMismatch);
            }
            if !task.start_observed {
                reasons.insert(R::MissingStart);
            }
            if task.terminal_evidence.is_empty() {
                reasons.insert(R::MissingTerminal);
            }
            if !task.anomalies.is_empty() {
                reasons.insert(R::LifecycleConflict);
            }
            if task.configuration_consistency == ConfigurationConsistency::Mixed {
                reasons.insert(R::MixedConfiguration);
            }
        }
    }
    let mut events = BTreeMap::new();
    for event in e.events {
        let n = &event.normalized_event;
        if n.event_id.is_empty()
            || n.task_id.as_deref() != Some(e.task_id)
            || n.session_id.as_deref() != e.session_id
            || n.source_instance_id != e.source_instance_id
        {
            reasons.insert(R::EventIdentityMismatch);
        }
        if let Some(previous) = events.insert(n.event_id.as_str(), event) {
            if previous.normalized_event != event.normalized_event
                || previous.effective_configuration != event.effective_configuration
                || previous.configuration_fingerprint != event.configuration_fingerprint
            {
                reasons.insert(R::ConflictingDuplicate);
            }
        }
    }
    match closure {
        None => {
            reasons.insert(R::ClosureUnproven);
        }
        Some(proof) => {
            let authorized = match (proof.contract, proof.provenance) {
                #[cfg(test)]
                (ClosureContract::SyntheticV1, ClosureProvenance::SyntheticAuthority) => true,
                _ => false,
            };
            let observed: BTreeSet<_> = events.keys().map(|id| (*id).to_owned()).collect();
            if !authorized
                || proof.source_instance_id != e.source_instance_id
                || proof.task_id != e.task_id
                || Some(proof.session_id.as_str()) != e.session_id
                || proof.committed_boundary == 0
                || Some(proof.committed_boundary) != e.boundary
                || proof.response_ids != observed
                || proof.response_ids.iter().any(String::is_empty)
            {
                reasons.insert(R::InvalidClosure);
            }
        }
    }
    let conflict = [
        R::SourceDiscontinuous,
        R::LifecycleConflict,
        R::EventIdentityMismatch,
        R::ConflictingDuplicate,
        R::InvalidClosure,
    ]
    .iter()
    .any(|r| reasons.contains(r));
    let status = if conflict {
        TaskCoverageStatus::Conflicting
    } else if reasons.contains(&R::MissingIdentity) {
        TaskCoverageStatus::Unavailable
    } else if reasons.is_empty() && closure.is_some() {
        TaskCoverageStatus::VerifiedComplete
    } else if reasons.len() == 1 && e.continuity == SourceContinuity::Continuous {
        TaskCoverageStatus::ContinuousButUnproven
    } else {
        TaskCoverageStatus::Partial
    };
    TaskCoverageAssessment {
        contract_version: COVERAGE_CONTRACT_VERSION,
        continuity: e.continuity,
        status,
        canonical_event_count: events.len(),
        reasons,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry::{ConfigurationState, TaskLifecycle, TaskTerminalEvidence};

    fn task() -> TaskState {
        TaskState {
            task_id: "task".into(),
            root_task_id: None,
            observed_root_task_ids: BTreeSet::new(),
            lifecycle: TaskLifecycle::Completed,
            start_observed: true,
            started_at: None,
            terminal_at: None,
            terminal_evidence: [TaskTerminalEvidence::Completed].into_iter().collect(),
            anomalies: BTreeSet::new(),
            configuration_override: ConfigurationState::default(),
            token_configuration_fingerprints: BTreeSet::new(),
            configuration_consistency: ConfigurationConsistency::Consistent,
        }
    }
    fn evidence(task: &TaskState) -> TaskCoverageEvidence<'_> {
        TaskCoverageEvidence {
            task_id: "task",
            session_id: Some("session"),
            source_instance_id: "source",
            boundary: Some(20),
            continuity: SourceContinuity::Continuous,
            task: Some(task),
            events: &[],
        }
    }
    #[test]
    fn continuous_terminal_is_not_complete_even_without_events() {
        let t = task();
        let a = assess_task_coverage(&evidence(&t));
        assert_eq!(a.status, TaskCoverageStatus::ContinuousButUnproven);
        assert_eq!(a.canonical_event_count, 0);
        assert_eq!(
            a.token_completeness(),
            TokenTelemetryCompleteness::Incomplete
        );
        assert!(a.reasons.contains(&TaskCoverageReason::ClosureUnproven));
    }
    #[test]
    fn missing_boundary_or_start_is_partial() {
        let mut t = task();
        t.start_observed = false;
        let mut e = evidence(&t);
        e.boundary = None;
        let a = assess_task_coverage(&e);
        assert_eq!(a.status, TaskCoverageStatus::Partial);
        assert!(a.reasons.contains(&TaskCoverageReason::MissingBoundary));
        assert!(a.reasons.contains(&TaskCoverageReason::MissingStart));
    }
    #[test]
    fn discontinuity_and_lifecycle_anomalies_fail_closed() {
        let mut t = task();
        t.anomalies
            .insert(crate::telemetry::TaskAnomaly::TokenAfterTerminal);
        let mut e = evidence(&t);
        e.continuity = SourceContinuity::Discontinuous;
        let a = assess_task_coverage(&e);
        assert_eq!(a.status, TaskCoverageStatus::Conflicting);
        assert!(a.reasons.contains(&TaskCoverageReason::SourceDiscontinuous));
        assert!(a.reasons.contains(&TaskCoverageReason::LifecycleConflict));
    }
    #[test]
    fn missing_identity_is_unavailable() {
        let t = task();
        let mut e = evidence(&t);
        e.session_id = None;
        assert_eq!(
            assess_task_coverage(&e).status,
            TaskCoverageStatus::Unavailable
        );
    }
    fn event(id: &str, total: u64) -> AttributedTokenEvent {
        use crate::telemetry::{
            ConfigurationIdentity, ConfigurationValue, NormalizedEventType, NormalizedTokenEvent,
            NormalizedTokenPayload, SchemaVersion, TokenCounters, TokenMetric, TokenPayloadKind,
        };
        let unavailable = ConfigurationValue::unavailable();
        AttributedTokenEvent {
            normalized_event: NormalizedTokenEvent {
                schema_version: SchemaVersion::V1,
                event_id: id.into(),
                event_type: NormalizedEventType::TokenCountersUpdated,
                source_instance_id: "source".into(),
                safe_cursor_id: None,
                event_at: "2026-01-01T00:00:00Z".into(),
                session_id: Some("session".into()),
                task_id: Some("task".into()),
                payload: NormalizedTokenPayload {
                    kind: TokenPayloadKind::TokenCounters,
                    token_counters: TokenCounters {
                        uncached_input: TokenMetric::unavailable(),
                        cached_input: TokenMetric::unavailable(),
                        output: TokenMetric::unavailable(),
                        reasoning_output: TokenMetric::unavailable(),
                        raw_total: TokenMetric::observed(total),
                    },
                },
            },
            effective_configuration: ConfigurationIdentity {
                plan: unavailable.clone(),
                model: unavailable.clone(),
                reasoning_level: unavailable.clone(),
                speed_mode: unavailable.clone(),
                codex_version: unavailable,
            },
            configuration_fingerprint: "config".into(),
            task_lifecycle: None,
        }
    }
    #[test]
    fn canonical_duplicates_deduplicate_and_conflicts_fail_closed() {
        let t = task();
        let first = event("one", 5);
        let second = event("two", 0);
        let events = vec![first.clone(), first.clone(), second];
        let mut e = evidence(&t);
        e.events = &events;
        let a = assess_task_coverage(&e);
        assert_eq!(a.canonical_event_count, 2);
        assert_eq!(a.status, TaskCoverageStatus::ContinuousButUnproven);
        let conflicts = vec![first, event("one", 6)];
        e.events = &conflicts;
        let a = assess_task_coverage(&e);
        assert_eq!(a.status, TaskCoverageStatus::Conflicting);
        assert!(a
            .reasons
            .contains(&TaskCoverageReason::ConflictingDuplicate));
    }
    #[test]
    fn wrong_generation_or_session_is_conflicting() {
        let t = task();
        let mut wrong = event("one", 1);
        wrong.normalized_event.source_instance_id = "other-generation".into();
        let events = vec![wrong];
        let mut e = evidence(&t);
        e.events = &events;
        let a = assess_task_coverage(&e);
        assert_eq!(a.status, TaskCoverageStatus::Conflicting);
        assert!(a
            .reasons
            .contains(&TaskCoverageReason::EventIdentityMismatch));
    }
    // This fixture is compiled only in tests and is not an adapter for rollout data.
    fn synthetic_closure(ids: &[&str]) -> TrustedTaskClosure {
        TrustedTaskClosure {
            contract: ClosureContract::SyntheticV1,
            source_instance_id: "source".into(),
            task_id: "task".into(),
            session_id: "session".into(),
            committed_boundary: 20,
            response_ids: ids.iter().map(|id| (*id).to_owned()).collect(),
            provenance: ClosureProvenance::SyntheticAuthority,
        }
    }
    #[test]
    fn synthetic_closure_admits_only_matching_exhaustive_evidence() {
        let t = task();
        let events = vec![event("one", 5), event("two", 0), event("one", 5)];
        let mut e = evidence(&t);
        e.events = &events;
        let proof = synthetic_closure(&["one", "two"]);
        let a = assess_with_closure(&e, Some(&proof));
        assert_eq!(a.status(), TaskCoverageStatus::VerifiedComplete);
        assert_eq!(a.canonical_event_count(), 2);
        assert_eq!(a.token_completeness(), TokenTelemetryCompleteness::Complete);
        assert_eq!(
            assess_task_coverage(&e).token_completeness(),
            TokenTelemetryCompleteness::Incomplete
        );
    }
    #[test]
    fn mismatched_proofs_fail_closed() {
        let t = task();
        let events = vec![event("one", 5)];
        let mut e = evidence(&t);
        e.events = &events;
        let mut proofs = Vec::new();
        let mut p = synthetic_closure(&["one"]);
        p.source_instance_id = "replacement".into();
        proofs.push(p);
        let mut p = synthetic_closure(&["one"]);
        p.session_id = "other".into();
        proofs.push(p);
        let mut p = synthetic_closure(&["one"]);
        p.task_id = "other".into();
        proofs.push(p);
        proofs.push(synthetic_closure(&["one", "missing"]));
        proofs.push(synthetic_closure(&[]));
        let mut p = synthetic_closure(&["one"]);
        p.committed_boundary = 19;
        proofs.push(p);
        let mut p = synthetic_closure(&["one"]);
        p.committed_boundary = 0;
        proofs.push(p);
        let mut p = synthetic_closure(&["one"]);
        p.contract = ClosureContract::Unavailable;
        proofs.push(p);
        let mut p = synthetic_closure(&["one"]);
        p.provenance = ClosureProvenance::Unavailable;
        proofs.push(p);
        for proof in &proofs {
            let a = assess_with_closure(&e, Some(proof));
            assert_eq!(a.status(), TaskCoverageStatus::Conflicting);
            assert!(a.reasons().contains(&TaskCoverageReason::InvalidClosure));
            assert_eq!(
                a.token_completeness(),
                TokenTelemetryCompleteness::Incomplete
            );
        }
        e.boundary = None;
        assert_ne!(
            assess_with_closure(&e, Some(&synthetic_closure(&["one"]))).status(),
            TaskCoverageStatus::VerifiedComplete
        );
    }
    #[test]
    fn closure_never_overrides_conflicts_or_unknown_continuity() {
        let mut t = task();
        let proof = synthetic_closure(&["one"]);
        let events = vec![event("one", 5), event("one", 6)];
        let mut e = evidence(&t);
        e.events = &events;
        assert_eq!(
            assess_with_closure(&e, Some(&proof)).status(),
            TaskCoverageStatus::Conflicting
        );
        let events = vec![event("one", 5)];
        e.events = &events;
        e.continuity = SourceContinuity::Discontinuous;
        assert_eq!(
            assess_with_closure(&e, Some(&proof)).status(),
            TaskCoverageStatus::Conflicting
        );
        e.continuity = SourceContinuity::Unknown;
        assert_ne!(
            assess_with_closure(&e, Some(&proof)).status(),
            TaskCoverageStatus::VerifiedComplete
        );
        t.anomalies
            .insert(crate::telemetry::TaskAnomaly::ConflictingTerminalEvidence);
        t.anomalies
            .insert(crate::telemetry::TaskAnomaly::TokenAfterTerminal);
        let mut e = evidence(&t);
        e.events = &events;
        assert_eq!(
            assess_with_closure(&e, Some(&proof)).status(),
            TaskCoverageStatus::Conflicting
        );
    }
}
