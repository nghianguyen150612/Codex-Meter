//! Trusted binding requirements for linking a run to existing Observation data.

use crate::telemetry::{
    EvidenceValidity, NormalizedObservation, ObservationLifecycle, QuotaMeterType, ReasonCode,
    ResetStatus,
};

use super::model::{parse_utc_timestamp, validate_configuration, ExperimentId, RunId};
use super::validation::{EvidenceBasis, IsolationClassification};
use super::BenchmarkError;

/// Existing Observation identity. Codex Meter emits `obs:` followed by 64
/// lowercase hexadecimal characters; benchmark IDs use a different type.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ObservationId(String);

impl ObservationId {
    pub fn parse(value: impl Into<String>) -> Result<Self, BenchmarkError> {
        let value = value.into();
        let Some(digest) = value.strip_prefix("obs:") else {
            return Err(BenchmarkError::ObservationMismatch);
        };
        if digest.len() != 64
            || !digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(BenchmarkError::ObservationMismatch);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Runtime-issued correlation evidence. The crate-private constructor is for
/// a trusted Rust runtime boundary after it has verified the run-to-Observation
/// association; an Observation ID by itself is not sufficient.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrustedRuntimeBinding {
    experiment_id: ExperimentId,
    run_id: RunId,
    repetition_index: u32,
    observation_id: ObservationId,
    isolation: IsolationClassification,
}

impl TrustedRuntimeBinding {
    #[allow(dead_code)] // Reserved for the authorized P025 runtime boundary.
    pub(crate) fn from_trusted_runtime(
        experiment_id: ExperimentId,
        run_id: RunId,
        repetition_index: u32,
        observation_id: impl Into<String>,
        isolation: IsolationClassification,
    ) -> Result<Self, BenchmarkError> {
        Ok(Self {
            experiment_id,
            run_id,
            repetition_index,
            observation_id: ObservationId::parse(observation_id)?,
            isolation,
        })
    }
}

/// A validated but not yet committed terminal Observation association.
/// Values are private so callers cannot manufacture linkage evidence.
#[derive(Debug)]
pub struct VerifiedObservationLink {
    pub(super) experiment_id: ExperimentId,
    pub(super) run_id: RunId,
    pub(super) repetition_index: u32,
    pub(super) observation_id: ObservationId,
    pub(super) linked_at: String,
    pub(super) finalized_at: String,
    pub(super) completion: MeasurementCompletionStatus,
    pub(super) isolation: IsolationClassification,
    pub(super) diagnostics: Vec<BenchmarkDiagnostic>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MeasurementCompletionStatus {
    MeasurementComplete,
    MeasurementIncomplete,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum BenchmarkDiagnostic {
    ConcurrentCodexOrWorkUsage,
    QuotaResetCrossing,
    MissingMeterEvidence,
    DelayedMeterStabilization,
    IncompleteTokenTelemetry,
    InterruptedExecution,
    InvalidObservationEvidence,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn verify_observation_link(
    experiment_id: &ExperimentId,
    run_id: &RunId,
    repetition_index: u32,
    run_started_at: &str,
    run_ended_at: &str,
    expected_configuration: &crate::telemetry::ConfigurationIdentity,
    target_meters: &[QuotaMeterType],
    binding: &TrustedRuntimeBinding,
    observation: &NormalizedObservation,
    linked_at: &str,
) -> Result<VerifiedObservationLink, BenchmarkError> {
    if binding.experiment_id != *experiment_id
        || binding.run_id != *run_id
        || binding.repetition_index != repetition_index
    {
        return Err(BenchmarkError::ObservationUnresolved);
    }
    let observation_id = ObservationId::parse(observation.observation_id.clone())?;
    if observation_id != binding.observation_id {
        return Err(BenchmarkError::ObservationUnresolved);
    }
    if !observation.identity_matches() {
        return Err(BenchmarkError::ObservationMismatch);
    }
    validate_configuration(&observation.configuration)
        .map_err(|_| BenchmarkError::ObservationMismatch)?;
    if observation.configuration != *expected_configuration {
        return Err(BenchmarkError::ConfigurationMismatch);
    }
    if observation.quota_evidence.five_hour.meter_type != QuotaMeterType::FiveHour
        || observation.quota_evidence.weekly.meter_type != QuotaMeterType::Weekly
    {
        return Err(BenchmarkError::ObservationMismatch);
    }

    let finalized_at = observation
        .timing
        .finalized_at
        .as_deref()
        .ok_or(BenchmarkError::ObservationUnresolved)?;
    let run_started =
        parse_utc_timestamp(run_started_at).map_err(|_| BenchmarkError::InvalidTimestamp)?;
    let run_ended =
        parse_utc_timestamp(run_ended_at).map_err(|_| BenchmarkError::InvalidTimestamp)?;
    let link_time = parse_utc_timestamp(linked_at).map_err(|_| BenchmarkError::InvalidTimestamp)?;
    let finalized =
        parse_utc_timestamp(finalized_at).map_err(|_| BenchmarkError::ObservationMismatch)?;
    if run_ended < run_started
        || finalized < run_ended
        || link_time < run_ended
        || link_time < finalized
    {
        return Err(BenchmarkError::InvalidTimeOrder);
    }

    let observed_started = observation
        .timing
        .started_at
        .as_deref()
        .map(parse_utc_timestamp)
        .transpose()
        .map_err(|_| BenchmarkError::ObservationMismatch)?;
    let observed_ended = observation
        .timing
        .ended_at
        .as_deref()
        .map(parse_utc_timestamp)
        .transpose()
        .map_err(|_| BenchmarkError::ObservationMismatch)?;
    if observed_started.is_some_and(|started| started < run_started || started > run_ended)
        || observed_ended.is_some_and(|ended| ended < run_started || ended > run_ended)
        || observed_started
            .zip(observed_ended)
            .is_some_and(|(started, ended)| ended < started)
        || observed_ended.is_some_and(|ended| finalized < ended)
    {
        return Err(BenchmarkError::InvalidTimeOrder);
    }

    let is_terminal = matches!(
        observation.lifecycle_state,
        ObservationLifecycle::Finalized
            | ObservationLifecycle::Incomplete
            | ObservationLifecycle::Invalid
    );
    if !is_terminal {
        return Err(BenchmarkError::ObservationUnresolved);
    }
    if observation.lifecycle_state == ObservationLifecycle::Finalized
        && (observation.token_evidence.status.validity != EvidenceValidity::Valid
            || observation.quota_evidence.five_hour.status.validity != EvidenceValidity::Valid
            || observation.quota_evidence.weekly.status.validity != EvidenceValidity::Valid)
    {
        return Err(BenchmarkError::ObservationMismatch);
    }

    let mut diagnostics = collect_diagnostics(observation, target_meters);
    let observation_reports_concurrency = observation
        .token_evidence
        .status
        .reason_codes
        .contains(&ReasonCode::ConcurrentUsagePossible)
        || all_meter_evidence(observation).any(|meter| {
            meter
                .status
                .reason_codes
                .contains(&ReasonCode::ConcurrentUsagePossible)
        });
    let isolation = if observation_reports_concurrency {
        IsolationClassification::PossibleConcurrentUsage {
            basis: EvidenceBasis::ObservationPipeline,
        }
    } else {
        binding.isolation
    };
    if matches!(
        isolation,
        IsolationClassification::PossibleConcurrentUsage { .. }
    ) {
        diagnostics.push(BenchmarkDiagnostic::ConcurrentCodexOrWorkUsage);
    }
    diagnostics.sort_unstable();
    diagnostics.dedup();

    let completion = if observation.lifecycle_state == ObservationLifecycle::Finalized
        && target_meter_evidence(observation, target_meters)
            .all(|meter| meter.status.validity == EvidenceValidity::Valid)
    {
        MeasurementCompletionStatus::MeasurementComplete
    } else {
        MeasurementCompletionStatus::MeasurementIncomplete
    };

    Ok(VerifiedObservationLink {
        experiment_id: experiment_id.clone(),
        run_id: run_id.clone(),
        repetition_index,
        observation_id,
        linked_at: linked_at.to_owned(),
        finalized_at: finalized_at.to_owned(),
        completion,
        isolation,
        diagnostics,
    })
}

fn target_meter_evidence<'a>(
    observation: &'a NormalizedObservation,
    target_meters: &'a [QuotaMeterType],
) -> impl Iterator<Item = &'a crate::telemetry::ObservationQuotaEvidence> + 'a {
    target_meters.iter().map(|meter| match meter {
        QuotaMeterType::FiveHour => &observation.quota_evidence.five_hour,
        QuotaMeterType::Weekly => &observation.quota_evidence.weekly,
    })
}

fn all_meter_evidence(
    observation: &NormalizedObservation,
) -> impl Iterator<Item = &crate::telemetry::ObservationQuotaEvidence> {
    [
        &observation.quota_evidence.five_hour,
        &observation.quota_evidence.weekly,
    ]
    .into_iter()
}

fn collect_diagnostics(
    observation: &NormalizedObservation,
    target_meters: &[QuotaMeterType],
) -> Vec<BenchmarkDiagnostic> {
    let mut result = Vec::new();
    let token_reasons = &observation.token_evidence.status.reason_codes;
    if matches!(
        observation.token_evidence.status.validity,
        EvidenceValidity::Incomplete | EvidenceValidity::Unavailable
    ) || token_reasons.contains(&ReasonCode::TelemetryIncomplete)
    {
        result.push(BenchmarkDiagnostic::IncompleteTokenTelemetry);
    }
    if token_reasons.contains(&ReasonCode::ProcessInterrupted) {
        result.push(BenchmarkDiagnostic::InterruptedExecution);
    }
    if token_reasons.contains(&ReasonCode::ConcurrentUsagePossible) {
        result.push(BenchmarkDiagnostic::ConcurrentCodexOrWorkUsage);
    }
    if observation.lifecycle_state == ObservationLifecycle::Invalid
        || observation.token_evidence.status.validity == EvidenceValidity::Invalid
    {
        result.push(BenchmarkDiagnostic::InvalidObservationEvidence);
    }

    for meter in target_meter_evidence(observation, target_meters) {
        if meter.reset_status == ResetStatus::Detected {
            result.push(BenchmarkDiagnostic::QuotaResetCrossing);
        }
        if meter.status.validity == EvidenceValidity::Unavailable {
            result.push(BenchmarkDiagnostic::MissingMeterEvidence);
        }
        if meter
            .status
            .reason_codes
            .contains(&ReasonCode::MeterUnstable)
        {
            result.push(BenchmarkDiagnostic::DelayedMeterStabilization);
        }
        if meter
            .status
            .reason_codes
            .contains(&ReasonCode::ConcurrentUsagePossible)
        {
            result.push(BenchmarkDiagnostic::ConcurrentCodexOrWorkUsage);
        }
        if meter.status.validity == EvidenceValidity::Invalid {
            result.push(BenchmarkDiagnostic::InvalidObservationEvidence);
        }
    }
    result.sort_unstable();
    result.dedup();
    result
}
