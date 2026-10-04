//! Replay-safe quota-window tracking and reset-safe percentage-point deltas.

use std::collections::BTreeSet;
use std::fmt;

use time::{format_description::well_known::Rfc3339, Duration, OffsetDateTime};

use super::identity;
use super::normalized::{
    ConfigurationValue, MetricAvailability, NormalizedQuotaSample, QuotaIdentityConfidence,
    QuotaIdentityProvenance, QuotaMeterType, QuotaWindowIdentity,
};

const FIVE_HOUR_MINUTES: i64 = 300;
const WEEKLY_MINUTES: i64 = 10_080;

/// Stateful independent tracker state for both supported quota meters.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QuotaTrackingState {
    pub five_hour: MeterTrackingState,
    pub weekly: MeterTrackingState,
}

impl Default for QuotaTrackingState {
    fn default() -> Self {
        Self {
            five_hour: MeterTrackingState::new(QuotaMeterType::FiveHour),
            weekly: MeterTrackingState::new(QuotaMeterType::Weekly),
        }
    }
}

/// State for one meter. It never contains raw provider payloads or account data.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MeterTrackingState {
    pub meter_type: QuotaMeterType,
    pub current_window: Option<TrackedQuotaWindow>,
    pub last_sample: Option<NormalizedQuotaSample>,
    seen_sample_ids: BTreeSet<String>,
}

impl MeterTrackingState {
    pub fn new(meter_type: QuotaMeterType) -> Self {
        Self {
            meter_type,
            current_window: None,
            last_sample: None,
            seen_sample_ids: BTreeSet::new(),
        }
    }
}

/// Explicit tracked identity, distinguishing provider evidence from local inference.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TrackedWindowIdentity {
    ObservedReset { observed_reset_at: String },
    LocallyInferred { local_window_id: String },
}

/// The current window and its observed evidence range.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrackedQuotaWindow {
    pub meter_type: QuotaMeterType,
    pub identity: TrackedWindowIdentity,
    pub identity_confidence: QuotaIdentityConfidence,
    pub first_sample_at: String,
    pub last_sample_at: String,
}

impl TrackedQuotaWindow {
    pub fn quota_window_identity(&self) -> QuotaWindowIdentity {
        let (local_window_id, observed_reset_at, identity_provenance) = match &self.identity {
            TrackedWindowIdentity::ObservedReset { observed_reset_at } => (
                None,
                Some(observed_reset_at.clone()),
                QuotaIdentityProvenance::ObservedReset,
            ),
            TrackedWindowIdentity::LocallyInferred { local_window_id } => (
                Some(local_window_id.clone()),
                None,
                QuotaIdentityProvenance::LocallyInferred,
            ),
        };
        QuotaWindowIdentity::Available {
            meter_type: self.meter_type,
            local_window_id,
            observed_reset_at,
            evidence_first_sample_at: Some(self.first_sample_at.clone()),
            evidence_last_sample_at: Some(self.last_sample_at.clone()),
            identity_provenance,
            identity_confidence: self.identity_confidence,
        }
    }
}

/// A validated, non-negative percentage-point value.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub struct NonNegativePercentagePoints(f64);

impl NonNegativePercentagePoints {
    pub fn new(value: f64) -> Result<Self, QuotaTrackingError> {
        if value.is_finite() && (0.0..=100.0).contains(&value) {
            Ok(Self(value))
        } else {
            Err(QuotaTrackingError::InvalidPercentagePoints)
        }
    }

    pub fn value(self) -> f64 {
        self.0
    }
}

/// Evidence that a pair belongs to one established window.
#[derive(Clone, Debug, PartialEq)]
pub struct SameWindowDelta {
    pub meter_type: QuotaMeterType,
    pub before_sample: NormalizedQuotaSample,
    pub after_sample: NormalizedQuotaSample,
    pub delta_percentage_points: NonNegativePercentagePoints,
    pub window: TrackedQuotaWindow,
}

/// Why a boundary was classified as observed or locally inferred.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BoundaryEvidence {
    ObservedReset,
    LocalInference,
}

/// Explicit reasons for contradictory meter behavior.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MeterInstabilityReason {
    SameObservedResetUsageDecreased,
    ObservedResetChangedBeforePreviousBoundary,
}

/// One typed result of advancing one meter.
#[derive(Clone, Debug, PartialEq)]
pub enum QuotaTrackingOutcome {
    BaselineEstablished {
        meter_type: QuotaMeterType,
        sample: NormalizedQuotaSample,
        window: TrackedQuotaWindow,
    },
    SameWindowDelta(SameWindowDelta),
    ResetDetected {
        meter_type: QuotaMeterType,
        before_sample: NormalizedQuotaSample,
        after_sample: NormalizedQuotaSample,
        previous_window: TrackedQuotaWindow,
        new_window: TrackedQuotaWindow,
        evidence: BoundaryEvidence,
    },
    InferredWindowBoundary {
        meter_type: QuotaMeterType,
        before_sample: NormalizedQuotaSample,
        after_sample: NormalizedQuotaSample,
        previous_window: TrackedQuotaWindow,
        new_window: TrackedQuotaWindow,
    },
    MeterUnstable {
        meter_type: QuotaMeterType,
        before_sample: NormalizedQuotaSample,
        after_sample: NormalizedQuotaSample,
        window: TrackedQuotaWindow,
        reason: MeterInstabilityReason,
    },
    PlanDiscontinuity {
        meter_type: QuotaMeterType,
        before_sample: NormalizedQuotaSample,
        after_sample: NormalizedQuotaSample,
        window: TrackedQuotaWindow,
    },
    DuplicateSample {
        meter_type: QuotaMeterType,
        sample_id: String,
    },
    OutOfOrderSample {
        meter_type: QuotaMeterType,
        sample_id: String,
        sampled_at: String,
        last_sampled_at: String,
    },
}

/// A transactional batch result.
#[derive(Clone, Debug, PartialEq)]
pub struct QuotaTrackingBatch {
    pub outcomes: Vec<QuotaTrackingOutcome>,
    pub next_state: QuotaTrackingState,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuotaTrackingError {
    MeterMismatch {
        expected: QuotaMeterType,
        actual: QuotaMeterType,
    },
    InvalidSampleTimestamp,
    InvalidObservedResetTimestamp,
    MissingUsedPercentage,
    InvalidUsedPercentage,
    InvalidPercentagePoints,
    InvalidWindowIdentityMeter {
        sample: QuotaMeterType,
        identity: QuotaMeterType,
    },
}

impl fmt::Display for QuotaTrackingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MeterMismatch { expected, actual } => {
                write!(
                    formatter,
                    "quota meter mismatch: expected {expected:?}, got {actual:?}"
                )
            }
            Self::InvalidSampleTimestamp => {
                formatter.write_str("quota sample timestamp is invalid")
            }
            Self::InvalidObservedResetTimestamp => {
                formatter.write_str("observed quota reset timestamp is invalid")
            }
            Self::MissingUsedPercentage => {
                formatter.write_str("quota used percentage is unavailable")
            }
            Self::InvalidUsedPercentage => {
                formatter.write_str("quota used percentage must be finite and between 0 and 100")
            }
            Self::InvalidPercentagePoints => {
                formatter.write_str("quota percentage points must be finite and between 0 and 100")
            }
            Self::InvalidWindowIdentityMeter { sample, identity } => write!(
                formatter,
                "quota window identity meter mismatch: sample {sample:?}, identity {identity:?}"
            ),
        }
    }
}

impl std::error::Error for QuotaTrackingError {}

/// Advances a meter-specific state by one normalized sample.
pub fn advance_meter(
    current: &mut MeterTrackingState,
    sample: &NormalizedQuotaSample,
) -> Result<QuotaTrackingOutcome, QuotaTrackingError> {
    validate_sample(current.meter_type, sample)?;

    if current.seen_sample_ids.contains(&sample.sample_id) {
        return Ok(QuotaTrackingOutcome::DuplicateSample {
            meter_type: current.meter_type,
            sample_id: sample.sample_id.clone(),
        });
    }

    if let Some(last_sample) = &current.last_sample {
        if timestamp(sample)? < timestamp(last_sample)? {
            return Ok(QuotaTrackingOutcome::OutOfOrderSample {
                meter_type: current.meter_type,
                sample_id: sample.sample_id.clone(),
                sampled_at: sample.sampled_at.clone(),
                last_sampled_at: last_sample.sampled_at.clone(),
            });
        }
    }

    let sample = sample.clone();
    let used = used_percent(&sample)?;
    let new_window = local_or_observed_window(&sample)?;
    let Some(previous_sample) = current.last_sample.clone() else {
        current.seen_sample_ids.insert(sample.sample_id.clone());
        current.last_sample = Some(sample.clone());
        current.current_window = Some(new_window.clone());
        return Ok(QuotaTrackingOutcome::BaselineEstablished {
            meter_type: current.meter_type,
            sample,
            window: new_window,
        });
    };
    let previous_window = current
        .current_window
        .clone()
        .expect("a last sample always has a current window");
    let previous_used = used_percent(&previous_sample)?;
    let previous_reset = observed_reset_at(&previous_sample)?
        .or_else(|| tracked_observed_reset_at(&previous_window));
    let current_reset = observed_reset_at(&sample)?;
    let sample_time = timestamp(&sample)?;
    let previous_time = timestamp(&previous_sample)?;

    current.seen_sample_ids.insert(sample.sample_id.clone());

    if has_observed_plan_change(&previous_sample, &sample) {
        let window = accept_as_baseline(current, sample.clone(), new_window);
        return Ok(QuotaTrackingOutcome::PlanDiscontinuity {
            meter_type: current.meter_type,
            before_sample: previous_sample,
            after_sample: sample,
            window,
        });
    }

    if let Some(previous_reset) = previous_reset {
        let previous_reset_time = parse_timestamp(previous_reset, true)?;
        if sample_time >= previous_reset_time {
            let window = accept_as_baseline(current, sample.clone(), new_window);
            return Ok(QuotaTrackingOutcome::ResetDetected {
                meter_type: current.meter_type,
                before_sample: previous_sample,
                after_sample: sample,
                previous_window,
                new_window: window,
                evidence: BoundaryEvidence::ObservedReset,
            });
        }

        if let Some(current_reset) = current_reset {
            if current_reset != previous_reset {
                let window = accept_as_baseline(current, sample.clone(), new_window);
                return Ok(QuotaTrackingOutcome::MeterUnstable {
                    meter_type: current.meter_type,
                    before_sample: previous_sample,
                    after_sample: sample,
                    window,
                    reason: MeterInstabilityReason::ObservedResetChangedBeforePreviousBoundary,
                });
            }
        }

        if used < previous_used {
            let window = accept_in_same_window(current, sample.clone(), previous_window);
            return Ok(QuotaTrackingOutcome::MeterUnstable {
                meter_type: current.meter_type,
                before_sample: previous_sample,
                after_sample: sample,
                window,
                reason: MeterInstabilityReason::SameObservedResetUsageDecreased,
            });
        }

        let window = accept_in_same_window(current, sample.clone(), previous_window);
        return Ok(QuotaTrackingOutcome::SameWindowDelta(SameWindowDelta {
            meter_type: current.meter_type,
            before_sample: previous_sample,
            after_sample: sample,
            delta_percentage_points: NonNegativePercentagePoints::new(used - previous_used)?,
            window,
        }));
    }

    if elapsed_at_least_nominal(current.meter_type, previous_time, sample_time) {
        let window = accept_as_baseline(current, sample.clone(), new_window);
        return Ok(QuotaTrackingOutcome::InferredWindowBoundary {
            meter_type: current.meter_type,
            before_sample: previous_sample,
            after_sample: sample,
            previous_window,
            new_window: window,
        });
    }

    if used < previous_used {
        let window = accept_as_baseline(current, sample.clone(), new_window);
        return Ok(QuotaTrackingOutcome::InferredWindowBoundary {
            meter_type: current.meter_type,
            before_sample: previous_sample,
            after_sample: sample,
            previous_window,
            new_window: window,
        });
    }

    let window = compatible_window_after_upgrade(previous_window, new_window, &sample);
    let window = accept_in_same_window(current, sample.clone(), window);
    Ok(QuotaTrackingOutcome::SameWindowDelta(SameWindowDelta {
        meter_type: current.meter_type,
        before_sample: previous_sample,
        after_sample: sample,
        delta_percentage_points: NonNegativePercentagePoints::new(used - previous_used)?,
        window,
    }))
}

/// Advances both independent meter states transactionally.
pub fn track_quota_samples(
    current_state: &QuotaTrackingState,
    samples: &[NormalizedQuotaSample],
) -> Result<QuotaTrackingBatch, QuotaTrackingError> {
    let mut next_state = current_state.clone();
    let mut outcomes = Vec::with_capacity(samples.len());
    for sample in samples {
        let meter = match sample.meter_type {
            QuotaMeterType::FiveHour => &mut next_state.five_hour,
            QuotaMeterType::Weekly => &mut next_state.weekly,
        };
        outcomes.push(advance_meter(meter, sample)?);
    }
    Ok(QuotaTrackingBatch {
        outcomes,
        next_state,
    })
}

fn validate_sample(
    expected_meter: QuotaMeterType,
    sample: &NormalizedQuotaSample,
) -> Result<(), QuotaTrackingError> {
    if sample.meter_type != expected_meter {
        return Err(QuotaTrackingError::MeterMismatch {
            expected: expected_meter,
            actual: sample.meter_type,
        });
    }
    timestamp(sample)?;
    used_percent(sample)?;
    if let QuotaWindowIdentity::Available {
        meter_type,
        identity_provenance,
        observed_reset_at,
        ..
    } = &sample.reset_evidence
    {
        if *meter_type != sample.meter_type {
            return Err(QuotaTrackingError::InvalidWindowIdentityMeter {
                sample: sample.meter_type,
                identity: *meter_type,
            });
        }
        if *identity_provenance == QuotaIdentityProvenance::ObservedReset
            && observed_reset_at.is_none()
        {
            return Err(QuotaTrackingError::InvalidObservedResetTimestamp);
        }
    }
    Ok(())
}

fn used_percent(sample: &NormalizedQuotaSample) -> Result<f64, QuotaTrackingError> {
    if sample.used_percent.availability != MetricAvailability::Available {
        return Err(QuotaTrackingError::MissingUsedPercentage);
    }
    let Some(value) = sample.used_percent.value else {
        return Err(QuotaTrackingError::MissingUsedPercentage);
    };
    if !value.is_finite() || !(0.0..=100.0).contains(&value) {
        return Err(QuotaTrackingError::InvalidUsedPercentage);
    }
    Ok(value)
}

fn timestamp(sample: &NormalizedQuotaSample) -> Result<OffsetDateTime, QuotaTrackingError> {
    parse_timestamp(&sample.sampled_at, false)
}

fn parse_timestamp(value: &str, reset: bool) -> Result<OffsetDateTime, QuotaTrackingError> {
    OffsetDateTime::parse(value, &Rfc3339).map_err(|_| {
        if reset {
            QuotaTrackingError::InvalidObservedResetTimestamp
        } else {
            QuotaTrackingError::InvalidSampleTimestamp
        }
    })
}

fn observed_reset_at(sample: &NormalizedQuotaSample) -> Result<Option<&str>, QuotaTrackingError> {
    match &sample.reset_evidence {
        QuotaWindowIdentity::Unavailable => Ok(None),
        QuotaWindowIdentity::Available {
            identity_provenance: QuotaIdentityProvenance::ObservedReset,
            observed_reset_at,
            ..
        } => {
            let observed_reset_at = observed_reset_at
                .as_deref()
                .ok_or(QuotaTrackingError::InvalidObservedResetTimestamp)?;
            parse_timestamp(observed_reset_at, true)?;
            Ok(Some(observed_reset_at))
        }
        QuotaWindowIdentity::Available { .. } => Ok(None),
    }
}

fn tracked_observed_reset_at(window: &TrackedQuotaWindow) -> Option<&str> {
    match &window.identity {
        TrackedWindowIdentity::ObservedReset { observed_reset_at } => Some(observed_reset_at),
        TrackedWindowIdentity::LocallyInferred { .. } => None,
    }
}

fn local_or_observed_window(
    sample: &NormalizedQuotaSample,
) -> Result<TrackedQuotaWindow, QuotaTrackingError> {
    let identity = match observed_reset_at(sample)? {
        Some(observed_reset_at) => TrackedWindowIdentity::ObservedReset {
            observed_reset_at: observed_reset_at.to_owned(),
        },
        None => TrackedWindowIdentity::LocallyInferred {
            local_window_id: identity::local_quota_window_id(
                sample.meter_type.as_str(),
                &sample.sample_id,
            ),
        },
    };
    Ok(TrackedQuotaWindow {
        meter_type: sample.meter_type,
        identity,
        identity_confidence: match observed_reset_at(sample)? {
            Some(_) => QuotaIdentityConfidence::High,
            None => QuotaIdentityConfidence::Medium,
        },
        first_sample_at: sample.sampled_at.clone(),
        last_sample_at: sample.sampled_at.clone(),
    })
}

fn compatible_window_after_upgrade(
    previous_window: TrackedQuotaWindow,
    current_window: TrackedQuotaWindow,
    sample: &NormalizedQuotaSample,
) -> TrackedQuotaWindow {
    match (&previous_window.identity, &current_window.identity) {
        (
            TrackedWindowIdentity::LocallyInferred { .. },
            TrackedWindowIdentity::ObservedReset { .. },
        ) => TrackedQuotaWindow {
            meter_type: sample.meter_type,
            identity: current_window.identity,
            identity_confidence: current_window.identity_confidence,
            first_sample_at: previous_window.first_sample_at,
            last_sample_at: sample.sampled_at.clone(),
        },
        _ => previous_window,
    }
}

fn accept_in_same_window(
    current: &mut MeterTrackingState,
    sample: NormalizedQuotaSample,
    mut window: TrackedQuotaWindow,
) -> TrackedQuotaWindow {
    window.last_sample_at = sample.sampled_at.clone();
    current.last_sample = Some(sample);
    current.current_window = Some(window.clone());
    window
}

fn accept_as_baseline(
    current: &mut MeterTrackingState,
    sample: NormalizedQuotaSample,
    window: TrackedQuotaWindow,
) -> TrackedQuotaWindow {
    current.last_sample = Some(sample);
    current.current_window = Some(window.clone());
    window
}

fn elapsed_at_least_nominal(
    meter_type: QuotaMeterType,
    previous: OffsetDateTime,
    current: OffsetDateTime,
) -> bool {
    let nominal_minutes = match meter_type {
        QuotaMeterType::FiveHour => FIVE_HOUR_MINUTES,
        QuotaMeterType::Weekly => WEEKLY_MINUTES,
    };
    current - previous >= Duration::minutes(nominal_minutes)
}

fn has_observed_plan_change(before: &NormalizedQuotaSample, after: &NormalizedQuotaSample) -> bool {
    observed_configuration_value(&before.configuration.plan)
        .zip(observed_configuration_value(&after.configuration.plan))
        .is_some_and(|(before, after)| before != after)
}

fn observed_configuration_value(value: &ConfigurationValue) -> Option<&str> {
    (value.availability == MetricAvailability::Available
        && value.provenance == super::normalized::MetricProvenance::Observed)
        .then_some(value.value.as_deref())
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry::normalized::{
        AcquisitionStatus, ConfigurationIdentity, MetricProvenance, PercentageMetric,
        QuotaSourceKind, SchemaVersion,
    };

    fn sample(
        meter_type: QuotaMeterType,
        sample_id: &str,
        sampled_at: &str,
        used: f64,
        reset_at: Option<&str>,
        plan: Option<&str>,
    ) -> NormalizedQuotaSample {
        let reset_evidence = match reset_at {
            Some(reset_at) => QuotaWindowIdentity::Available {
                meter_type,
                local_window_id: None,
                observed_reset_at: Some(reset_at.to_owned()),
                evidence_first_sample_at: None,
                evidence_last_sample_at: None,
                identity_provenance: QuotaIdentityProvenance::ObservedReset,
                identity_confidence: super::super::normalized::QuotaIdentityConfidence::High,
            },
            None => QuotaWindowIdentity::Unavailable,
        };
        NormalizedQuotaSample {
            schema_version: SchemaVersion::V1,
            sample_id: sample_id.to_owned(),
            meter_type,
            sampled_at: sampled_at.to_owned(),
            used_percent: PercentageMetric {
                availability: MetricAvailability::Available,
                value: Some(used),
                provenance: MetricProvenance::Observed,
            },
            remaining_percent: PercentageMetric::derived(100.0 - used),
            reset_evidence,
            configuration: ConfigurationIdentity {
                plan: plan.map_or_else(
                    ConfigurationValue::unavailable,
                    ConfigurationValue::observed,
                ),
                model: ConfigurationValue::unavailable(),
                reasoning_level: ConfigurationValue::unavailable(),
                speed_mode: ConfigurationValue::unavailable(),
                codex_version: ConfigurationValue::unavailable(),
            },
            acquisition_status: AcquisitionStatus::Succeeded,
            source_kind: QuotaSourceKind::LocalMeter,
        }
    }

    fn delta(outcome: &QuotaTrackingOutcome) -> f64 {
        match outcome {
            QuotaTrackingOutcome::SameWindowDelta(delta) => delta.delta_percentage_points.value(),
            _ => panic!("expected same-window delta"),
        }
    }

    #[test]
    fn observed_reset_crossing_rejects_canonical_regression() {
        let mut state = MeterTrackingState::new(QuotaMeterType::FiveHour);
        advance_meter(
            &mut state,
            &sample(
                QuotaMeterType::FiveHour,
                "before",
                "2026-10-04T09:00:00Z",
                97.0,
                Some("2026-10-04T10:00:00Z"),
                Some("plus"),
            ),
        )
        .unwrap();
        let outcome = advance_meter(
            &mut state,
            &sample(
                QuotaMeterType::FiveHour,
                "after",
                "2026-10-04T10:01:00Z",
                4.0,
                Some("2026-10-04T15:00:00Z"),
                Some("plus"),
            ),
        )
        .unwrap();
        assert!(matches!(
            outcome,
            QuotaTrackingOutcome::ResetDetected { .. }
        ));
        assert_eq!(state.last_sample.as_ref().unwrap().sample_id, "after");
    }

    #[test]
    fn same_observed_reset_accepts_positive_and_zero_delta() {
        let mut state = MeterTrackingState::new(QuotaMeterType::FiveHour);
        advance_meter(
            &mut state,
            &sample(
                QuotaMeterType::FiveHour,
                "a",
                "2026-10-04T09:00:00Z",
                42.0,
                Some("2026-10-04T14:00:00Z"),
                Some("plus"),
            ),
        )
        .unwrap();
        assert_eq!(
            delta(
                &advance_meter(
                    &mut state,
                    &sample(
                        QuotaMeterType::FiveHour,
                        "b",
                        "2026-10-04T09:01:00Z",
                        49.5,
                        Some("2026-10-04T14:00:00Z"),
                        Some("plus"),
                    ),
                )
                .unwrap(),
            ),
            7.5
        );
        assert_eq!(
            delta(
                &advance_meter(
                    &mut state,
                    &sample(
                        QuotaMeterType::FiveHour,
                        "c",
                        "2026-10-04T09:02:00Z",
                        49.5,
                        Some("2026-10-04T14:00:00Z"),
                        Some("plus"),
                    ),
                )
                .unwrap(),
            ),
            0.0
        );
    }

    #[test]
    fn distinct_samples_at_same_timestamp_are_allowed() {
        let mut state = MeterTrackingState::new(QuotaMeterType::FiveHour);
        advance_meter(
            &mut state,
            &sample(
                QuotaMeterType::FiveHour,
                "same-time-a",
                "2026-10-04T09:00:00Z",
                20.0,
                Some("2026-10-04T14:00:00Z"),
                Some("plus"),
            ),
        )
        .unwrap();
        let outcome = advance_meter(
            &mut state,
            &sample(
                QuotaMeterType::FiveHour,
                "same-time-b",
                "2026-10-04T09:00:00Z",
                21.0,
                Some("2026-10-04T14:00:00Z"),
                Some("plus"),
            ),
        )
        .unwrap();
        assert_eq!(delta(&outcome), 1.0);
    }

    #[test]
    fn same_reset_decrease_is_instability_not_reset() {
        let mut state = MeterTrackingState::new(QuotaMeterType::FiveHour);
        advance_meter(
            &mut state,
            &sample(
                QuotaMeterType::FiveHour,
                "a",
                "2026-10-04T09:00:00Z",
                40.0,
                Some("2026-10-04T14:00:00Z"),
                Some("plus"),
            ),
        )
        .unwrap();
        let outcome = advance_meter(
            &mut state,
            &sample(
                QuotaMeterType::FiveHour,
                "b",
                "2026-10-04T09:01:00Z",
                35.0,
                Some("2026-10-04T14:00:00Z"),
                Some("plus"),
            ),
        )
        .unwrap();
        assert!(matches!(
            outcome,
            QuotaTrackingOutcome::MeterUnstable {
                reason: MeterInstabilityReason::SameObservedResetUsageDecreased,
                ..
            }
        ));
    }

    #[test]
    fn known_boundary_overrides_higher_usage() {
        let mut state = MeterTrackingState::new(QuotaMeterType::FiveHour);
        advance_meter(
            &mut state,
            &sample(
                QuotaMeterType::FiveHour,
                "a",
                "2026-10-04T09:00:00Z",
                10.0,
                Some("2026-10-04T10:00:00Z"),
                Some("plus"),
            ),
        )
        .unwrap();
        let outcome = advance_meter(
            &mut state,
            &sample(
                QuotaMeterType::FiveHour,
                "b",
                "2026-10-04T10:30:00Z",
                90.0,
                Some("2026-10-04T15:30:00Z"),
                Some("plus"),
            ),
        )
        .unwrap();
        assert!(matches!(
            outcome,
            QuotaTrackingOutcome::ResetDetected { .. }
        ));
    }

    #[test]
    fn reset_correction_before_boundary_is_unstable() {
        let mut state = MeterTrackingState::new(QuotaMeterType::FiveHour);
        advance_meter(
            &mut state,
            &sample(
                QuotaMeterType::FiveHour,
                "a",
                "2026-10-04T11:00:00Z",
                20.0,
                Some("2026-10-04T15:00:00Z"),
                Some("plus"),
            ),
        )
        .unwrap();
        let outcome = advance_meter(
            &mut state,
            &sample(
                QuotaMeterType::FiveHour,
                "b",
                "2026-10-04T12:00:00Z",
                21.0,
                Some("2026-10-04T16:00:00Z"),
                Some("plus"),
            ),
        )
        .unwrap();
        assert!(matches!(
            outcome,
            QuotaTrackingOutcome::MeterUnstable {
                reason: MeterInstabilityReason::ObservedResetChangedBeforePreviousBoundary,
                ..
            }
        ));
    }

    #[test]
    fn local_monotonic_sequence_has_medium_confidence_and_deltas() {
        let mut state = MeterTrackingState::new(QuotaMeterType::FiveHour);
        let first = sample(
            QuotaMeterType::FiveHour,
            "a",
            "2026-10-04T00:00:00Z",
            10.0,
            None,
            None,
        );
        let first_outcome = advance_meter(&mut state, &first).unwrap();
        assert!(matches!(
            first_outcome,
            QuotaTrackingOutcome::BaselineEstablished {
                window: TrackedQuotaWindow {
                    identity: TrackedWindowIdentity::LocallyInferred { .. },
                    ..
                },
                ..
            }
        ));
        assert_eq!(
            delta(
                &advance_meter(
                    &mut state,
                    &sample(
                        QuotaMeterType::FiveHour,
                        "b",
                        "2026-10-04T01:00:00Z",
                        15.0,
                        None,
                        None,
                    ),
                )
                .unwrap(),
            ),
            5.0
        );
        assert_eq!(
            delta(
                &advance_meter(
                    &mut state,
                    &sample(
                        QuotaMeterType::FiveHour,
                        "c",
                        "2026-10-04T02:00:00Z",
                        17.0,
                        None,
                        None,
                    ),
                )
                .unwrap(),
            ),
            2.0
        );
        assert!(matches!(
            state.current_window.as_ref().unwrap().identity,
            TrackedWindowIdentity::LocallyInferred { .. }
        ));
        assert_eq!(
            state.current_window.as_ref().unwrap().identity_confidence,
            QuotaIdentityConfidence::Medium
        );
    }

    #[test]
    fn local_decrease_starts_new_window() {
        let mut state = MeterTrackingState::new(QuotaMeterType::FiveHour);
        for (id, time, used) in [
            ("a", "2026-10-04T00:00:00Z", 10.0),
            ("b", "2026-10-04T01:00:00Z", 20.0),
        ] {
            advance_meter(
                &mut state,
                &sample(QuotaMeterType::FiveHour, id, time, used, None, None),
            )
            .unwrap();
        }
        let outcome = advance_meter(
            &mut state,
            &sample(
                QuotaMeterType::FiveHour,
                "c",
                "2026-10-04T02:00:00Z",
                3.0,
                None,
                None,
            ),
        )
        .unwrap();
        assert!(matches!(
            outcome,
            QuotaTrackingOutcome::InferredWindowBoundary { .. }
        ));
        assert_eq!(state.last_sample.as_ref().unwrap().sample_id, "c");
    }

    #[test]
    fn local_gap_starts_new_window_even_when_usage_increases() {
        let mut state = MeterTrackingState::new(QuotaMeterType::FiveHour);
        advance_meter(
            &mut state,
            &sample(
                QuotaMeterType::FiveHour,
                "a",
                "2026-10-04T00:00:00Z",
                10.0,
                None,
                None,
            ),
        )
        .unwrap();
        let outcome = advance_meter(
            &mut state,
            &sample(
                QuotaMeterType::FiveHour,
                "b",
                "2026-10-04T06:00:00Z",
                80.0,
                None,
                None,
            ),
        )
        .unwrap();
        assert!(matches!(
            outcome,
            QuotaTrackingOutcome::InferredWindowBoundary { .. }
        ));
    }

    #[test]
    fn plan_change_isolated_but_missing_plan_is_not_a_change() {
        let mut state = MeterTrackingState::new(QuotaMeterType::FiveHour);
        advance_meter(
            &mut state,
            &sample(
                QuotaMeterType::FiveHour,
                "a",
                "2026-10-04T00:00:00Z",
                20.0,
                Some("2026-10-04T05:00:00Z"),
                Some("plus"),
            ),
        )
        .unwrap();
        let outcome = advance_meter(
            &mut state,
            &sample(
                QuotaMeterType::FiveHour,
                "b",
                "2026-10-04T00:01:00Z",
                25.0,
                Some("2026-10-04T05:00:00Z"),
                Some("pro"),
            ),
        )
        .unwrap();
        assert!(matches!(
            outcome,
            QuotaTrackingOutcome::PlanDiscontinuity { .. }
        ));

        let mut missing_plan = MeterTrackingState::new(QuotaMeterType::FiveHour);
        advance_meter(
            &mut missing_plan,
            &sample(
                QuotaMeterType::FiveHour,
                "c",
                "2026-10-04T00:00:00Z",
                20.0,
                Some("2026-10-04T05:00:00Z"),
                None,
            ),
        )
        .unwrap();
        let outcome = advance_meter(
            &mut missing_plan,
            &sample(
                QuotaMeterType::FiveHour,
                "d",
                "2026-10-04T00:01:00Z",
                25.0,
                Some("2026-10-04T05:00:00Z"),
                Some("plus"),
            ),
        )
        .unwrap();
        assert_eq!(delta(&outcome), 5.0);
    }

    #[test]
    fn replay_and_out_of_order_do_not_mutate_state() {
        let mut state = MeterTrackingState::new(QuotaMeterType::FiveHour);
        let first = sample(
            QuotaMeterType::FiveHour,
            "a",
            "2026-10-04T12:00:00Z",
            20.0,
            None,
            None,
        );
        advance_meter(&mut state, &first).unwrap();
        let replay_before = state.clone();
        assert!(matches!(
            advance_meter(&mut state, &first).unwrap(),
            QuotaTrackingOutcome::DuplicateSample { .. }
        ));
        assert_eq!(state, replay_before);
        let before_out_of_order = state.clone();
        assert!(matches!(
            advance_meter(
                &mut state,
                &sample(
                    QuotaMeterType::FiveHour,
                    "older",
                    "2026-10-04T11:00:00Z",
                    21.0,
                    None,
                    None,
                ),
            )
            .unwrap(),
            QuotaTrackingOutcome::OutOfOrderSample { .. }
        ));
        assert_eq!(state, before_out_of_order);
    }

    #[test]
    fn missing_current_reset_can_delta_before_known_boundary_but_not_after() {
        let mut state = MeterTrackingState::new(QuotaMeterType::FiveHour);
        advance_meter(
            &mut state,
            &sample(
                QuotaMeterType::FiveHour,
                "before",
                "2026-10-04T09:00:00Z",
                20.0,
                Some("2026-10-04T10:00:00Z"),
                Some("plus"),
            ),
        )
        .unwrap();
        assert_eq!(
            delta(
                &advance_meter(
                    &mut state,
                    &sample(
                        QuotaMeterType::FiveHour,
                        "before-boundary",
                        "2026-10-04T09:30:00Z",
                        25.0,
                        None,
                        Some("plus"),
                    ),
                )
                .unwrap(),
            ),
            5.0
        );
        let outcome = advance_meter(
            &mut state,
            &sample(
                QuotaMeterType::FiveHour,
                "after-boundary",
                "2026-10-04T10:01:00Z",
                26.0,
                None,
                Some("plus"),
            ),
        )
        .unwrap();
        assert!(matches!(
            outcome,
            QuotaTrackingOutcome::ResetDetected { .. }
        ));
    }

    #[test]
    fn locally_inferred_identity_upgrades_to_observed_reset() {
        let mut state = MeterTrackingState::new(QuotaMeterType::FiveHour);
        advance_meter(
            &mut state,
            &sample(
                QuotaMeterType::FiveHour,
                "local",
                "2026-10-04T09:00:00Z",
                20.0,
                None,
                None,
            ),
        )
        .unwrap();
        let outcome = advance_meter(
            &mut state,
            &sample(
                QuotaMeterType::FiveHour,
                "observed",
                "2026-10-04T09:30:00Z",
                25.0,
                Some("2026-10-04T14:00:00Z"),
                None,
            ),
        )
        .unwrap();
        assert_eq!(delta(&outcome), 5.0);
        let window = match outcome {
            QuotaTrackingOutcome::SameWindowDelta(delta) => delta.window,
            _ => unreachable!(),
        };
        assert!(matches!(
            window.identity,
            TrackedWindowIdentity::ObservedReset { .. }
        ));
        assert_eq!(window.identity_confidence, QuotaIdentityConfidence::High);
        assert_eq!(window.first_sample_at, "2026-10-04T09:00:00Z");
    }

    #[test]
    fn ordered_replay_is_deterministic() {
        let samples = vec![
            sample(
                QuotaMeterType::FiveHour,
                "a",
                "2026-10-04T00:00:00Z",
                10.0,
                None,
                None,
            ),
            sample(
                QuotaMeterType::FiveHour,
                "b",
                "2026-10-04T01:00:00Z",
                15.0,
                None,
                None,
            ),
            sample(
                QuotaMeterType::FiveHour,
                "c",
                "2026-10-04T02:00:00Z",
                17.0,
                None,
                None,
            ),
        ];
        let first = track_quota_samples(&QuotaTrackingState::default(), &samples).unwrap();
        let second = track_quota_samples(&QuotaTrackingState::default(), &samples).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn meter_specific_api_rejects_cross_meter_samples() {
        let mut state = MeterTrackingState::new(QuotaMeterType::FiveHour);
        let error = advance_meter(
            &mut state,
            &sample(
                QuotaMeterType::Weekly,
                "weekly",
                "2026-10-04T00:00:00Z",
                1.0,
                None,
                None,
            ),
        )
        .unwrap_err();
        assert_eq!(
            error,
            QuotaTrackingError::MeterMismatch {
                expected: QuotaMeterType::FiveHour,
                actual: QuotaMeterType::Weekly,
            }
        );
        assert_eq!(state, MeterTrackingState::new(QuotaMeterType::FiveHour));
    }

    #[test]
    fn meters_are_independent_and_batches_are_transactional() {
        let initial = QuotaTrackingState::default();
        let samples = vec![
            sample(
                QuotaMeterType::FiveHour,
                "five-a",
                "2026-10-04T09:00:00Z",
                97.0,
                Some("2026-10-04T10:00:00Z"),
                Some("plus"),
            ),
            sample(
                QuotaMeterType::Weekly,
                "week-a",
                "2026-10-04T09:00:00Z",
                31.0,
                Some("2026-10-11T09:00:00Z"),
                Some("plus"),
            ),
            sample(
                QuotaMeterType::FiveHour,
                "five-b",
                "2026-10-04T10:01:00Z",
                4.0,
                Some("2026-10-04T15:01:00Z"),
                Some("plus"),
            ),
            sample(
                QuotaMeterType::Weekly,
                "week-b",
                "2026-10-04T10:00:00Z",
                32.0,
                Some("2026-10-11T09:00:00Z"),
                Some("plus"),
            ),
        ];
        let batch = track_quota_samples(&initial, &samples).unwrap();
        assert!(matches!(
            batch.outcomes[2],
            QuotaTrackingOutcome::ResetDetected { .. }
        ));
        assert_eq!(delta(&batch.outcomes[3]), 1.0);
        assert_eq!(initial, QuotaTrackingState::default());

        let invalid = sample(
            QuotaMeterType::FiveHour,
            "invalid",
            "not-a-timestamp",
            1.0,
            None,
            None,
        );
        assert!(track_quota_samples(&batch.next_state, &[invalid]).is_err());
        assert_eq!(
            batch
                .next_state
                .five_hour
                .last_sample
                .as_ref()
                .unwrap()
                .sample_id,
            "five-b"
        );
    }

    #[test]
    fn local_window_id_is_stable() {
        let mut state = MeterTrackingState::new(QuotaMeterType::FiveHour);
        let outcome = advance_meter(
            &mut state,
            &sample(
                QuotaMeterType::FiveHour,
                "fixed-anchor",
                "2026-10-04T00:00:00Z",
                1.0,
                None,
                None,
            ),
        )
        .unwrap();
        let window = match outcome {
            QuotaTrackingOutcome::BaselineEstablished { window, .. } => window,
            _ => panic!("expected baseline"),
        };
        assert_eq!(
            window.identity,
            TrackedWindowIdentity::LocallyInferred {
                local_window_id:
                    "window:5c5d1f8584a6d22886af16420b2d3ca75c0edec975e3b45e8c6e0f5f6072587d"
                        .to_owned(),
            }
        );
        assert_eq!(window.identity_confidence, QuotaIdentityConfidence::Medium);
        assert_eq!(
            window.quota_window_identity(),
            QuotaWindowIdentity::Available {
                meter_type: QuotaMeterType::FiveHour,
                local_window_id: Some(
                    "window:5c5d1f8584a6d22886af16420b2d3ca75c0edec975e3b45e8c6e0f5f6072587d"
                        .to_owned(),
                ),
                observed_reset_at: None,
                evidence_first_sample_at: Some("2026-10-04T00:00:00Z".to_owned()),
                evidence_last_sample_at: Some("2026-10-04T00:00:00Z".to_owned()),
                identity_provenance: QuotaIdentityProvenance::LocallyInferred,
                identity_confidence: QuotaIdentityConfidence::Medium,
            }
        );
    }
}
