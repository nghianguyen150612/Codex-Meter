//! Typed preflight evidence and fail-closed outcome evaluation.

use crate::telemetry::QuotaMeterType;

use super::model::{parse_utc_timestamp, BenchmarkPlan};
use super::BenchmarkError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EvidenceBasis {
    ObservedRuntime,
    ObservationPipeline,
    AuthorizedTestFixture,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GateBlockReason {
    ConfigurationNotReady,
    TelemetrySourceUnavailable,
    QuotaMeterUnavailable,
    QuotaWindowDiscontinuity,
    PossibleConcurrentUsage,
    ExecutionBoundaryUnavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnknownEvidenceReason {
    EvidenceUnavailable,
    ConfigurationNotObserved,
    TelemetrySourceNotObserved,
    QuotaMeterNotObserved,
    QuotaWindowContinuityNotObserved,
    IsolationNotVerified,
    ExecutionBoundaryNotObserved,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GateEvidence {
    Verified { basis: EvidenceBasis },
    Blocked { reason: GateBlockReason },
    Unknown { reason: UnknownEvidenceReason },
}

impl GateEvidence {
    pub const fn verified(basis: EvidenceBasis) -> Self {
        Self::Verified { basis }
    }

    pub const fn blocked(reason: GateBlockReason) -> Self {
        Self::Blocked { reason }
    }

    pub const fn unknown(reason: UnknownEvidenceReason) -> Self {
        Self::Unknown { reason }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IsolationClassification {
    VerifiedIsolated { basis: EvidenceBasis },
    PossibleConcurrentUsage { basis: EvidenceBasis },
    Unknown,
}

impl IsolationClassification {
    pub const fn verified(basis: EvidenceBasis) -> Self {
        Self::VerifiedIsolated { basis }
    }

    pub const fn possible_concurrent_usage(basis: EvidenceBasis) -> Self {
        Self::PossibleConcurrentUsage { basis }
    }

    pub const fn unknown() -> Self {
        Self::Unknown
    }

    pub const fn is_verified_isolated(self) -> bool {
        matches!(self, Self::VerifiedIsolated { .. })
    }

    fn as_gate_evidence(self) -> GateEvidence {
        match self {
            Self::VerifiedIsolated { basis } => GateEvidence::Verified { basis },
            Self::PossibleConcurrentUsage { .. } => GateEvidence::Blocked {
                reason: GateBlockReason::PossibleConcurrentUsage,
            },
            Self::Unknown => GateEvidence::Unknown {
                reason: UnknownEvidenceReason::IsolationNotVerified,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreflightOutcome {
    Ready,
    Blocked,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MeterPreflightEvidence {
    meter: QuotaMeterType,
    evidence: GateEvidence,
}

impl MeterPreflightEvidence {
    pub const fn new(meter: QuotaMeterType, evidence: GateEvidence) -> Self {
        Self { meter, evidence }
    }

    pub const fn meter(self) -> QuotaMeterType {
        self.meter
    }

    pub const fn evidence(self) -> GateEvidence {
        self.evidence
    }
}

/// Snapshot of typed readiness evidence. Its execution-boundary gate confirms
/// that a future runtime can enforce a single-run authorization; it does not
/// itself authorize any quota-consuming action.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BenchmarkPreflight {
    plan: BenchmarkPlan,
    configuration_readiness: GateEvidence,
    telemetry_source: GateEvidence,
    quota_meter_availability: Vec<MeterPreflightEvidence>,
    quota_window_continuity: Vec<MeterPreflightEvidence>,
    isolation: IsolationClassification,
    execution_boundary_readiness: GateEvidence,
    evaluated_at: String,
    outcome: PreflightOutcome,
}

impl BenchmarkPreflight {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        plan: &BenchmarkPlan,
        configuration_readiness: GateEvidence,
        telemetry_source: GateEvidence,
        quota_meter_availability: Vec<MeterPreflightEvidence>,
        quota_window_continuity: Vec<MeterPreflightEvidence>,
        isolation: IsolationClassification,
        execution_boundary_readiness: GateEvidence,
        evaluated_at: impl Into<String>,
    ) -> Result<Self, BenchmarkError> {
        let evaluated_at = evaluated_at.into();
        if parse_utc_timestamp(&evaluated_at).is_err() {
            return Err(BenchmarkError::InvalidTimestamp);
        }
        validate_meter_coverage(plan, &quota_meter_availability)?;
        validate_meter_coverage(plan, &quota_window_continuity)?;
        let mut result = Self {
            plan: plan.clone(),
            configuration_readiness,
            telemetry_source,
            quota_meter_availability,
            quota_window_continuity,
            isolation,
            execution_boundary_readiness,
            evaluated_at,
            outcome: PreflightOutcome::Unknown,
        };
        result.outcome = result.evaluate_outcome();
        Ok(result)
    }

    pub fn unknown_for_plan(
        plan: &BenchmarkPlan,
        evaluated_at: impl Into<String>,
    ) -> Result<Self, BenchmarkError> {
        let unknown = GateEvidence::unknown(UnknownEvidenceReason::EvidenceUnavailable);
        let meter_unknown = GateEvidence::unknown(UnknownEvidenceReason::QuotaMeterNotObserved);
        let window_unknown =
            GateEvidence::unknown(UnknownEvidenceReason::QuotaWindowContinuityNotObserved);
        Self::new(
            plan,
            GateEvidence::unknown(UnknownEvidenceReason::ConfigurationNotObserved),
            GateEvidence::unknown(UnknownEvidenceReason::TelemetrySourceNotObserved),
            plan.target_quota_meters()
                .iter()
                .copied()
                .map(|meter| MeterPreflightEvidence::new(meter, meter_unknown))
                .collect(),
            plan.target_quota_meters()
                .iter()
                .copied()
                .map(|meter| MeterPreflightEvidence::new(meter, window_unknown))
                .collect(),
            IsolationClassification::Unknown,
            unknown,
            evaluated_at,
        )
    }

    pub fn outcome(&self) -> PreflightOutcome {
        self.outcome
    }

    pub(super) fn belongs_to(&self, plan: &BenchmarkPlan) -> bool {
        self.plan == *plan
    }

    pub fn evaluated_at(&self) -> &str {
        &self.evaluated_at
    }

    pub fn isolation(&self) -> IsolationClassification {
        self.isolation
    }

    pub fn quota_meter_availability(&self) -> &[MeterPreflightEvidence] {
        &self.quota_meter_availability
    }

    pub fn quota_window_continuity(&self) -> &[MeterPreflightEvidence] {
        &self.quota_window_continuity
    }

    pub(crate) fn contains_test_fixture_evidence(&self) -> bool {
        let is_fixture = |evidence: GateEvidence| {
            matches!(
                evidence,
                GateEvidence::Verified {
                    basis: EvidenceBasis::AuthorizedTestFixture
                }
            )
        };
        is_fixture(self.configuration_readiness)
            || is_fixture(self.telemetry_source)
            || is_fixture(self.execution_boundary_readiness)
            || matches!(
                self.isolation,
                IsolationClassification::VerifiedIsolated {
                    basis: EvidenceBasis::AuthorizedTestFixture
                }
            )
            || self
                .quota_meter_availability
                .iter()
                .chain(self.quota_window_continuity.iter())
                .any(|entry| is_fixture(entry.evidence))
    }

    fn evaluate_outcome(&self) -> PreflightOutcome {
        let mut has_unknown = false;
        let evidence = [
            self.configuration_readiness,
            self.telemetry_source,
            self.execution_boundary_readiness,
            self.isolation.as_gate_evidence(),
        ]
        .into_iter()
        .chain(
            self.quota_meter_availability
                .iter()
                .map(|entry| entry.evidence),
        )
        .chain(
            self.quota_window_continuity
                .iter()
                .map(|entry| entry.evidence),
        );

        for item in evidence {
            match item {
                GateEvidence::Verified { .. } => {}
                GateEvidence::Blocked { .. } => return PreflightOutcome::Blocked,
                GateEvidence::Unknown { .. } => has_unknown = true,
            }
        }
        if has_unknown {
            PreflightOutcome::Unknown
        } else {
            PreflightOutcome::Ready
        }
    }
}

fn validate_meter_coverage(
    plan: &BenchmarkPlan,
    evidence: &[MeterPreflightEvidence],
) -> Result<(), BenchmarkError> {
    if evidence.len() != plan.target_quota_meters().len() {
        return Err(BenchmarkError::InvalidPreflight);
    }
    for (index, item) in evidence.iter().enumerate() {
        if !plan.target_quota_meters().contains(&item.meter)
            || evidence[index + 1..]
                .iter()
                .any(|later| later.meter == item.meter)
        {
            return Err(BenchmarkError::InvalidPreflight);
        }
    }
    Ok(())
}
