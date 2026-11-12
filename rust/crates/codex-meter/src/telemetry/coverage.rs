//! Conservative task coverage assessment. Continuity is not closure.
//!
//! No audited production source currently provides a completeness contract.
//! This evaluator therefore never manufactures `Complete` from local evidence.

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
    pub contract_version: u32,
    pub continuity: SourceContinuity,
    pub status: TaskCoverageStatus,
    pub canonical_event_count: usize,
    pub reasons: BTreeSet<TaskCoverageReason>,
}

impl TaskCoverageAssessment {
    /// P013 remains authoritative for checked token arithmetic and validity.
    pub fn token_completeness(&self) -> TokenTelemetryCompleteness {
        // No production closure contract has been audited. Keep this fail closed
        // even if future changes accidentally construct an assessment directly.
        TokenTelemetryCompleteness::Incomplete
    }
}

pub fn assess_task_coverage(e: &TaskCoverageEvidence<'_>) -> TaskCoverageAssessment {
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
    reasons.insert(R::ClosureUnproven);
    let conflict = [
        R::SourceDiscontinuous,
        R::LifecycleConflict,
        R::EventIdentityMismatch,
        R::ConflictingDuplicate,
    ]
    .iter()
    .any(|r| reasons.contains(r));
    let status = if conflict {
        TaskCoverageStatus::Conflicting
    } else if reasons.contains(&R::MissingIdentity) {
        TaskCoverageStatus::Unavailable
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
}
