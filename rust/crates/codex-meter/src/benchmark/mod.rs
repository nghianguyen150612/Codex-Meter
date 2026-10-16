//! Provider-independent controlled benchmark experiment contracts.

pub mod evidence;
pub mod lifecycle;
pub mod model;
pub mod validation;

#[cfg(test)]
mod tests;

pub use evidence::{
    BenchmarkDiagnostic, MeasurementCompletionStatus, ObservationId, TrustedRuntimeBinding,
    VerifiedObservationLink,
};
pub use lifecycle::{
    BenchmarkExperiment, BenchmarkObservationClaims, BenchmarkRun, BenchmarkRunState,
    ExplicitRunAuthorization,
};
pub use model::{
    AuthorizationId, BenchmarkPlan, ExperimentId, IdentityValidationError, PlanValidationError,
    RunId, WorkloadKey, BENCHMARK_FRAMEWORK_VERSION, MAX_PLANNED_RUNS,
};
pub use validation::{
    BenchmarkPreflight, EvidenceBasis, GateBlockReason, GateEvidence, IsolationClassification,
    MeterPreflightEvidence, PreflightOutcome, UnknownEvidenceReason,
};

use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BenchmarkError {
    InvalidPlan(PlanValidationError),
    InvalidPreflight,
    InvalidTimestamp,
    InvalidTimeOrder,
    InvalidTransition,
    PreflightBlocked,
    PreflightUnknown,
    AuthorizationRequired,
    AuthorizationScopeMismatch,
    DuplicateRun,
    DuplicateRepetition,
    RunLimitExceeded,
    RunNotFound,
    ObservationMismatch,
    ObservationUnresolved,
    ConfigurationMismatch,
    DuplicateObservation,
    EvidenceIncomplete,
    ConflictingReplay,
}

impl fmt::Display for BenchmarkError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidPlan(error) => return error.fmt(formatter),
            Self::InvalidPreflight => "benchmark preflight evidence does not match the plan",
            Self::InvalidTimestamp => "benchmark timestamp is invalid",
            Self::InvalidTimeOrder => "benchmark event timestamps are inconsistent",
            Self::InvalidTransition => "benchmark lifecycle transition is not allowed",
            Self::PreflightBlocked => "benchmark preflight is blocked",
            Self::PreflightUnknown => "benchmark preflight evidence is unknown",
            Self::AuthorizationRequired => "explicit authorization is required for this run",
            Self::AuthorizationScopeMismatch => "authorization does not match this run",
            Self::DuplicateRun => "benchmark run ID is already in use",
            Self::DuplicateRepetition => "benchmark repetition index is already in use",
            Self::RunLimitExceeded => "benchmark plan run count has been reached",
            Self::RunNotFound => "benchmark run does not exist",
            Self::ObservationMismatch => "Observation evidence does not match its contract",
            Self::ObservationUnresolved => "trusted Observation linkage is unavailable",
            Self::ConfigurationMismatch => "Observation configuration does not match the plan",
            Self::DuplicateObservation => "Observation is already linked to another benchmark run",
            Self::EvidenceIncomplete => "benchmark measurement evidence is incomplete",
            Self::ConflictingReplay => "transition replay conflicts with accepted evidence",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for BenchmarkError {}
