//! Provider-independent assembly of deterministic v1 observations.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{ser::SerializeStruct, Serialize, Serializer};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

use super::assembly::AttributedTokenEvent;
use super::identity;
use super::normalized::{
    ConfigurationIdentity, ConfigurationValue, MetricAvailability, MetricProvenance,
    NormalizedQuotaSample, QuotaMeterType, QuotaWindowIdentity, TokenCounters, TokenMetric,
};
use super::quota_reconciliation::{
    AttributionRisk, BaselineUnavailableReason, MeterReconciliationState, TaskQuotaReconciliation,
};
use super::token_normalization::{validate_timestamp, TimestampErrorReason};

const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceValidity {
    Valid,
    Incomplete,
    Invalid,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub enum QualityGrade {
    A,
    B,
    C,
    D,
    X,
}

impl QualityGrade {
    fn max(self, other: Self) -> Self {
        std::cmp::max(self, other)
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasonCode {
    QuotaResetCrossed,
    MeterUnavailable,
    MeterUnstable,
    TelemetryIncomplete,
    ConcurrentUsagePossible,
    ProcessInterrupted,
    SourceAcquisitionFailure,
    UnknownReason,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResetStatus {
    NotDetected,
    Detected,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct EvidenceStatus {
    pub validity: EvidenceValidity,
    pub quality: QualityGrade,
    pub reason_codes: Vec<ReasonCode>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ObservationTiming {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finalized_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ObservationTokenEvidence {
    pub status: EvidenceStatus,
    pub raw_token_counters: TokenCounters,
}

#[derive(Clone, Debug, PartialEq)]
pub struct QuotaSampleSnapshot {
    pub sample_id: String,
    pub sampled_at: String,
    pub used_percent: super::normalized::PercentageMetric,
    pub remaining_percent: super::normalized::PercentageMetric,
}

impl Eq for QuotaSampleSnapshot {}

impl Serialize for QuotaSampleSnapshot {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("QuotaSampleSnapshot", 4)?;
        state.serialize_field("sample_id", &self.sample_id)?;
        state.serialize_field("sampled_at", &self.sampled_at)?;
        state.serialize_field("used_percent", &self.used_percent)?;
        state.serialize_field("remaining_percent", &self.remaining_percent)?;
        state.end()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ObservationQuotaEvidence {
    pub meter_type: QuotaMeterType,
    pub status: EvidenceStatus,
    pub before_sample: Option<QuotaSampleSnapshot>,
    pub after_sample: Option<QuotaSampleSnapshot>,
    pub delta_percentage_points: Option<f64>,
    pub window_identity: QuotaWindowIdentity,
    pub reset_status: ResetStatus,
}

pub type ObservationMeterEvidence = ObservationQuotaEvidence;

impl Eq for ObservationQuotaEvidence {}

impl Serialize for ObservationQuotaEvidence {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if self.reset_status == ResetStatus::Detected && self.delta_percentage_points.is_some() {
            return Err(serde::ser::Error::custom(
                "reset-detected quota evidence cannot contain a delta",
            ));
        }
        let mut state = serializer.serialize_struct("ObservationQuotaEvidence", 7)?;
        state.serialize_field("meter_type", &self.meter_type)?;
        state.serialize_field("status", &self.status)?;
        state.serialize_field("before_sample", &self.before_sample)?;
        state.serialize_field("after_sample", &self.after_sample)?;
        if let Some(delta) = self.delta_percentage_points {
            state.serialize_field("delta_percentage_points", &delta)?;
        }
        state.serialize_field("window_identity", &self.window_identity)?;
        state.serialize_field("reset_status", &self.reset_status)?;
        state.end()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ObservationQuotaEvidenceSet {
    pub five_hour: ObservationQuotaEvidence,
    pub weekly: ObservationQuotaEvidence,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct NormalizedObservation {
    pub schema_version: super::normalized::SchemaVersion,
    pub observation_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_instance_id: Option<String>,
    pub lifecycle_state: ObservationLifecycle,
    pub timing: ObservationTiming,
    pub configuration: ConfigurationIdentity,
    pub summary_quality: QualityGrade,
    pub token_evidence: ObservationTokenEvidence,
    pub quota_evidence: ObservationQuotaEvidenceSet,
}

pub type Observation = NormalizedObservation;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationLifecycle {
    Detected,
    Active,
    TaskEnded,
    AwaitingMeter,
    Reconciling,
    Finalized,
    Incomplete,
    Invalid,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TokenTelemetryCompleteness {
    Complete,
    Incomplete,
    ProcessInterrupted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IsolationEvidence {
    ControlledBenchmark,
    IsolatedNormalTask,
    PossibleConcurrentUsage,
    UnknownExternalUsage,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObservationInput {
    pub reconciliation: TaskQuotaReconciliation,
    pub attributed_token_events: Vec<AttributedTokenEvent>,
    pub token_completeness: TokenTelemetryCompleteness,
    pub isolation: IsolationEvidence,
    pub evaluated_at: Option<String>,
}

impl ObservationInput {
    pub fn new(
        reconciliation: TaskQuotaReconciliation,
        attributed_token_events: Vec<AttributedTokenEvent>,
        token_completeness: TokenTelemetryCompleteness,
        isolation: IsolationEvidence,
        evaluated_at: Option<String>,
    ) -> Self {
        Self {
            reconciliation,
            attributed_token_events,
            token_completeness,
            isolation,
            evaluated_at,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObservationError {
    InvalidTimestamp(TimestampErrorReason),
    TaskEndedBeforeStart,
    DurationOverflow,
    MissingTaskIdentity,
    TokenEventTaskMismatch,
    ConflictingTokenEvent,
    TokenCounterOverflow,
}

impl fmt::Display for ObservationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidTimestamp(reason) => {
                return write!(formatter, "invalid observation timestamp: {reason}")
            }
            Self::TaskEndedBeforeStart => "task ended before it started",
            Self::DurationOverflow => "task duration exceeded the safe integer range",
            Self::MissingTaskIdentity => "observation task identity is missing",
            Self::TokenEventTaskMismatch => "token event belongs to a different task",
            Self::ConflictingTokenEvent => "duplicate token event ID has conflicting evidence",
            Self::TokenCounterOverflow => "token aggregate exceeded the safe integer range",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for ObservationError {}

pub fn build_observation(
    input: &ObservationInput,
) -> Result<NormalizedObservation, ObservationError> {
    let task_id = if input.reconciliation.task.task_id.is_empty() {
        return Err(ObservationError::MissingTaskIdentity);
    } else {
        input.reconciliation.task.task_id.as_str()
    };
    validate_optional_timestamp(input.reconciliation.task.started_at.as_deref())?;
    validate_optional_timestamp(input.reconciliation.task.ended_at.as_deref())?;
    validate_optional_timestamp(input.evaluated_at.as_deref())?;

    let timing = build_timing(
        &input.reconciliation.task.started_at,
        &input.reconciliation.task.ended_at,
        &input.evaluated_at,
    )?;
    let (events, source_instance_id) = deduplicate_events(task_id, &input.attributed_token_events)?;
    let mixed_configuration = has_mixed_configuration(&events);
    let (mut configuration, plan_conflict) = build_configuration(&events, &input.reconciliation);
    if plan_conflict {
        configuration.plan = ConfigurationValue::unavailable();
    }
    let mut token = aggregate_tokens(
        &events,
        input.token_completeness,
        mixed_configuration,
        source_instance_id.is_none(),
    )?;
    let base_quality = isolation_quality(input.isolation, &input.reconciliation.attribution_risk);
    let concurrent_risk = matches!(
        input.isolation,
        IsolationEvidence::PossibleConcurrentUsage | IsolationEvidence::UnknownExternalUsage
    ) || matches!(
        input.reconciliation.attribution_risk,
        AttributionRisk::KnownLocalOverlap { .. }
    );
    if token.status.validity == EvidenceValidity::Valid {
        token.status.quality = token.status.quality.max(base_quality);
    }
    if concurrent_risk {
        token
            .status
            .reason_codes
            .push(ReasonCode::ConcurrentUsagePossible);
        token.status.reason_codes = sorted_reasons(std::mem::take(&mut token.status.reason_codes));
    }
    let mut five_hour = build_meter_evidence(
        &input.reconciliation.five_hour,
        base_quality,
        &input.reconciliation.attribution_risk,
        concurrent_risk,
    );
    let mut weekly = build_meter_evidence(
        &input.reconciliation.weekly,
        base_quality,
        &input.reconciliation.attribution_risk,
        concurrent_risk,
    );
    if plan_conflict {
        degrade_status(&mut five_hour.status, ReasonCode::TelemetryIncomplete);
        degrade_status(&mut weekly.status, ReasonCode::TelemetryIncomplete);
    }
    let lifecycle = lifecycle_for(
        &input.reconciliation,
        &token.status,
        &five_hour.status,
        &weekly.status,
    );
    let summary_quality = worst_quality([
        token.status.quality,
        five_hour.status.quality,
        weekly.status.quality,
    ]);
    let finalized_at = matches!(
        lifecycle,
        ObservationLifecycle::Finalized
            | ObservationLifecycle::Incomplete
            | ObservationLifecycle::Invalid
    )
    .then(|| input.evaluated_at.clone())
    .flatten();
    Ok(NormalizedObservation {
        schema_version: super::normalized::SchemaVersion::V1,
        observation_id: identity::observation_id(
            task_id,
            input.reconciliation.task.session_id.as_deref(),
            input.reconciliation.task.ended_at.as_deref(),
        ),
        task_id: Some(input.reconciliation.task.task_id.clone()),
        session_id: input.reconciliation.task.session_id.clone(),
        source_instance_id,
        lifecycle_state: lifecycle,
        timing: ObservationTiming {
            started_at: input.reconciliation.task.started_at.clone(),
            ended_at: input.reconciliation.task.ended_at.clone(),
            finalized_at,
            duration_ms: timing,
        },
        configuration,
        summary_quality,
        token_evidence: token,
        quota_evidence: ObservationQuotaEvidenceSet { five_hour, weekly },
    })
}

fn build_timing(
    start: &Option<String>,
    end: &Option<String>,
    _evaluated_at: &Option<String>,
) -> Result<Option<u64>, ObservationError> {
    let (Some(start), Some(end)) = (start.as_deref(), end.as_deref()) else {
        return Ok(None);
    };
    let started = OffsetDateTime::parse(start, &Rfc3339)
        .map_err(|_| ObservationError::InvalidTimestamp(TimestampErrorReason::InvalidTime))?;
    let ended = OffsetDateTime::parse(end, &Rfc3339)
        .map_err(|_| ObservationError::InvalidTimestamp(TimestampErrorReason::InvalidTime))?;
    let duration = ended - started;
    let milliseconds = duration.whole_milliseconds();
    if milliseconds < 0 {
        return Err(ObservationError::TaskEndedBeforeStart);
    }
    u64::try_from(milliseconds)
        .ok()
        .filter(|value| *value <= MAX_SAFE_INTEGER)
        .ok_or(ObservationError::DurationOverflow)
        .map(Some)
}

fn validate_optional_timestamp(value: Option<&str>) -> Result<(), ObservationError> {
    let Some(value) = value else {
        return Ok(());
    };
    validate_timestamp(value).map_err(ObservationError::InvalidTimestamp)?;
    OffsetDateTime::parse(value, &Rfc3339)
        .map(|_| ())
        .map_err(|_| ObservationError::InvalidTimestamp(TimestampErrorReason::InvalidTime))
}

fn deduplicate_events<'a>(
    task_id: &str,
    events: &'a [AttributedTokenEvent],
) -> Result<(Vec<&'a AttributedTokenEvent>, Option<String>), ObservationError> {
    let mut by_id: BTreeMap<&str, &AttributedTokenEvent> = BTreeMap::new();
    for event in events {
        if event.normalized_event.task_id.as_deref() != Some(task_id) {
            return Err(ObservationError::TokenEventTaskMismatch);
        }
        if let Some(previous) = by_id.get(event.normalized_event.event_id.as_str()) {
            if previous.normalized_event != event.normalized_event
                || previous.effective_configuration != event.effective_configuration
                || previous.configuration_fingerprint != event.configuration_fingerprint
            {
                return Err(ObservationError::ConflictingTokenEvent);
            }
        } else {
            by_id.insert(event.normalized_event.event_id.as_str(), event);
        }
    }
    let mut source_ids = BTreeSet::new();
    for event in by_id.values() {
        source_ids.insert(event.normalized_event.source_instance_id.clone());
    }
    let source =
        (source_ids.len() == 1).then(|| source_ids.into_iter().next().expect("one source"));
    Ok((by_id.into_values().collect(), source))
}

fn has_mixed_configuration(events: &[&AttributedTokenEvent]) -> bool {
    events
        .windows(2)
        .any(|pair| pair[0].effective_configuration != pair[1].effective_configuration)
}

fn build_configuration(
    events: &[&AttributedTokenEvent],
    reconciliation: &TaskQuotaReconciliation,
) -> (ConfigurationIdentity, bool) {
    let mut configuration = events
        .first()
        .map_or_else(unavailable_configuration, |event| {
            event.effective_configuration.clone()
        });
    if has_mixed_configuration(events) {
        configuration = unavailable_configuration();
    }
    let mut plans = BTreeSet::new();
    for sample in meter_samples(&reconciliation.five_hour)
        .into_iter()
        .chain(meter_samples(&reconciliation.weekly))
    {
        if let Some(plan) = sample.configuration.plan.value.as_deref() {
            plans.insert(plan.to_owned());
        }
    }
    let plan_conflict = plans.len() > 1;
    if configuration.plan.availability == MetricAvailability::Unavailable && plans.len() == 1 {
        configuration.plan = ConfigurationValue {
            availability: MetricAvailability::Available,
            value: plans.into_iter().next(),
            provenance: MetricProvenance::Observed,
        };
    }
    (configuration, plan_conflict)
}

fn unavailable_configuration() -> ConfigurationIdentity {
    ConfigurationIdentity {
        plan: ConfigurationValue::unavailable(),
        model: ConfigurationValue::unavailable(),
        reasoning_level: ConfigurationValue::unavailable(),
        speed_mode: ConfigurationValue::unavailable(),
        codex_version: ConfigurationValue::unavailable(),
    }
}

fn meter_samples(
    meter: &super::quota_reconciliation::MeterReconciliation,
) -> Vec<NormalizedQuotaSample> {
    match &meter.state {
        MeterReconciliationState::Stable { evidence } => vec![
            evidence.before_sample.clone(),
            evidence.after_sample.clone(),
        ],
        MeterReconciliationState::ResetCrossed { evidence } => vec![
            evidence.before_sample.clone(),
            evidence.boundary_sample.clone(),
        ],
        MeterReconciliationState::MeterUnstable { evidence } => vec![
            evidence.before_sample.clone(),
            evidence.relevant_sample.clone(),
        ],
        MeterReconciliationState::PlanDiscontinuity { evidence } => vec![
            evidence.before_sample.clone(),
            evidence.relevant_sample.clone(),
        ],
        MeterReconciliationState::TimedOut { evidence }
        | MeterReconciliationState::AcquisitionFailed { evidence } => evidence
            .before_sample
            .iter()
            .chain(evidence.latest_candidate.iter())
            .chain(evidence.last_successful_sample.iter())
            .cloned()
            .collect(),
        _ => meter.before_sample.clone().into_iter().collect(),
    }
}

fn aggregate_tokens(
    events: &[&AttributedTokenEvent],
    completeness: TokenTelemetryCompleteness,
    mixed: bool,
    source_conflict: bool,
) -> Result<ObservationTokenEvidence, ObservationError> {
    let mut counters = TokenCounters {
        uncached_input: TokenMetric::unavailable(),
        cached_input: TokenMetric::unavailable(),
        output: TokenMetric::unavailable(),
        reasoning_output: TokenMetric::unavailable(),
        raw_total: TokenMetric::unavailable(),
    };
    if events.is_empty() {
        return Ok(ObservationTokenEvidence {
            status: EvidenceStatus::new(
                EvidenceValidity::Unavailable,
                QualityGrade::D,
                [ReasonCode::TelemetryIncomplete],
            ),
            raw_token_counters: counters,
        });
    }
    counters.uncached_input = aggregate_metric(events, metric_uncached)?;
    counters.cached_input = aggregate_metric(events, metric_cached)?;
    counters.output = aggregate_metric(events, metric_output)?;
    counters.reasoning_output = aggregate_metric(events, metric_reasoning)?;
    counters.raw_total = aggregate_metric(events, metric_raw_total)?;
    let raw_available = counters.raw_total.availability == MetricAvailability::Available;
    let mut status = if !raw_available || mixed || source_conflict {
        EvidenceStatus::new(
            EvidenceValidity::Incomplete,
            QualityGrade::D,
            [ReasonCode::TelemetryIncomplete],
        )
    } else {
        EvidenceStatus::new(EvidenceValidity::Valid, QualityGrade::B, [])
    };
    if completeness != TokenTelemetryCompleteness::Complete {
        status.validity = EvidenceValidity::Incomplete;
        status.quality = QualityGrade::D;
        status.reason_codes.push(
            if completeness == TokenTelemetryCompleteness::ProcessInterrupted {
                ReasonCode::ProcessInterrupted
            } else {
                ReasonCode::TelemetryIncomplete
            },
        );
    }
    status.reason_codes = sorted_reasons(status.reason_codes);
    Ok(ObservationTokenEvidence {
        status,
        raw_token_counters: counters,
    })
}

fn aggregate_metric(
    events: &[&AttributedTokenEvent],
    selector: fn(&TokenCounters) -> &TokenMetric,
) -> Result<TokenMetric, ObservationError> {
    let metrics: Vec<&TokenMetric> = events
        .iter()
        .map(|event| selector(&event.normalized_event.payload.token_counters))
        .collect();
    if metrics
        .iter()
        .any(|metric| metric.availability != MetricAvailability::Available)
    {
        return Ok(TokenMetric::unavailable());
    }
    let mut sum = 0_u64;
    for metric in metrics {
        let Some(value) = metric.value else {
            return Ok(TokenMetric::unavailable());
        };
        sum = match sum.checked_add(value) {
            Some(value) if value <= MAX_SAFE_INTEGER => value,
            _ => return Err(ObservationError::TokenCounterOverflow),
        };
    }
    Ok(TokenMetric {
        availability: MetricAvailability::Available,
        value: Some(sum),
        provenance: MetricProvenance::Derived,
    })
}

fn metric_uncached(counters: &TokenCounters) -> &TokenMetric {
    &counters.uncached_input
}
fn metric_cached(counters: &TokenCounters) -> &TokenMetric {
    &counters.cached_input
}
fn metric_output(counters: &TokenCounters) -> &TokenMetric {
    &counters.output
}
fn metric_reasoning(counters: &TokenCounters) -> &TokenMetric {
    &counters.reasoning_output
}
fn metric_raw_total(counters: &TokenCounters) -> &TokenMetric {
    &counters.raw_total
}

impl EvidenceStatus {
    fn new<const N: usize>(
        validity: EvidenceValidity,
        quality: QualityGrade,
        reasons: [ReasonCode; N],
    ) -> Self {
        Self {
            validity,
            quality,
            reason_codes: sorted_reasons(reasons.into_iter().collect()),
        }
    }
}

fn build_meter_evidence(
    meter: &super::quota_reconciliation::MeterReconciliation,
    base_quality: QualityGrade,
    risk: &AttributionRisk,
    concurrent_risk: bool,
) -> ObservationQuotaEvidence {
    let meter_type = meter.meter_type;
    match &meter.state {
        MeterReconciliationState::Stable { evidence } => ObservationQuotaEvidence {
            meter_type,
            status: status_with_context(
                EvidenceValidity::Valid,
                base_quality,
                risk,
                concurrent_risk,
            ),
            before_sample: Some(snapshot(&evidence.before_sample)),
            after_sample: Some(snapshot(&evidence.after_sample)),
            delta_percentage_points: Some(evidence.delta_percentage_points.value()),
            window_identity: evidence.window_identity.clone(),
            reset_status: ResetStatus::NotDetected,
        },
        MeterReconciliationState::ResetCrossed { evidence } => ObservationQuotaEvidence {
            meter_type,
            status: EvidenceStatus::new(
                EvidenceValidity::Invalid,
                QualityGrade::X,
                [ReasonCode::QuotaResetCrossed],
            ),
            before_sample: Some(snapshot(&evidence.before_sample)),
            after_sample: Some(snapshot(&evidence.boundary_sample)),
            delta_percentage_points: None,
            window_identity: QuotaWindowIdentity::Unavailable,
            reset_status: ResetStatus::Detected,
        },
        MeterReconciliationState::NoBaseline { reason } => {
            let mut reasons = vec![ReasonCode::MeterUnavailable];
            if *reason == BaselineUnavailableReason::MissingTaskStart {
                reasons.push(ReasonCode::TelemetryIncomplete);
            }
            ObservationQuotaEvidence {
                meter_type,
                status: EvidenceStatus {
                    validity: EvidenceValidity::Unavailable,
                    quality: QualityGrade::D,
                    reason_codes: sorted_reasons(reasons),
                },
                before_sample: None,
                after_sample: None,
                delta_percentage_points: None,
                window_identity: QuotaWindowIdentity::Unavailable,
                reset_status: ResetStatus::Unavailable,
            }
        }
        MeterReconciliationState::MeterUnstable { evidence } => ObservationQuotaEvidence {
            meter_type,
            status: EvidenceStatus::new(
                EvidenceValidity::Incomplete,
                QualityGrade::D,
                [ReasonCode::MeterUnstable],
            ),
            before_sample: Some(snapshot(&evidence.before_sample)),
            after_sample: Some(snapshot(&evidence.relevant_sample)),
            delta_percentage_points: None,
            window_identity: QuotaWindowIdentity::Unavailable,
            reset_status: ResetStatus::Unavailable,
        },
        MeterReconciliationState::PlanDiscontinuity { evidence } => ObservationQuotaEvidence {
            meter_type,
            status: EvidenceStatus::new(
                EvidenceValidity::Invalid,
                QualityGrade::X,
                [ReasonCode::UnknownReason],
            ),
            before_sample: Some(snapshot(&evidence.before_sample)),
            after_sample: Some(snapshot(&evidence.relevant_sample)),
            delta_percentage_points: None,
            window_identity: QuotaWindowIdentity::Unavailable,
            reset_status: ResetStatus::Unavailable,
        },
        MeterReconciliationState::TimedOut { evidence } => {
            timed_out_evidence(meter_type, evidence, false)
        }
        MeterReconciliationState::AcquisitionFailed { evidence } => {
            timed_out_evidence(meter_type, evidence, true)
        }
        MeterReconciliationState::AwaitingAfterSample => pending_evidence(meter, false),
        MeterReconciliationState::Reconciling { .. } => pending_evidence(meter, true),
    }
}

fn timed_out_evidence(
    meter_type: QuotaMeterType,
    evidence: &super::quota_reconciliation::TimedOutEvidence,
    acquisition_failed: bool,
) -> ObservationQuotaEvidence {
    let has_usable_after =
        evidence.latest_candidate.is_some() || evidence.last_successful_sample.is_some();
    let mut reasons = if acquisition_failed {
        vec![ReasonCode::SourceAcquisitionFailure]
    } else if has_usable_after {
        vec![ReasonCode::MeterUnstable]
    } else {
        vec![ReasonCode::MeterUnavailable]
    };
    if acquisition_failed && !has_usable_after {
        reasons.push(ReasonCode::MeterUnavailable);
    }
    ObservationQuotaEvidence {
        meter_type,
        status: EvidenceStatus {
            validity: EvidenceValidity::Incomplete,
            quality: QualityGrade::D,
            reason_codes: sorted_reasons(reasons),
        },
        before_sample: evidence.before_sample.as_ref().map(snapshot),
        after_sample: evidence
            .latest_candidate
            .as_ref()
            .or(evidence.last_successful_sample.as_ref())
            .map(snapshot),
        delta_percentage_points: None,
        window_identity: QuotaWindowIdentity::Unavailable,
        reset_status: ResetStatus::Unavailable,
    }
}

fn pending_evidence(
    meter: &super::quota_reconciliation::MeterReconciliation,
    _reconciling: bool,
) -> ObservationQuotaEvidence {
    ObservationQuotaEvidence {
        meter_type: meter.meter_type,
        status: EvidenceStatus::new(
            EvidenceValidity::Incomplete,
            QualityGrade::D,
            [ReasonCode::MeterUnavailable],
        ),
        before_sample: meter.before_sample.as_ref().map(snapshot),
        after_sample: match &meter.state {
            MeterReconciliationState::Reconciling {
                candidate: Some(candidate),
            } => Some(snapshot(&candidate.sample)),
            _ => None,
        },
        delta_percentage_points: None,
        window_identity: QuotaWindowIdentity::Unavailable,
        reset_status: ResetStatus::Unavailable,
    }
}

fn snapshot(sample: &NormalizedQuotaSample) -> QuotaSampleSnapshot {
    QuotaSampleSnapshot {
        sample_id: sample.sample_id.clone(),
        sampled_at: sample.sampled_at.clone(),
        used_percent: sample.used_percent.clone(),
        remaining_percent: sample.remaining_percent.clone(),
    }
}

fn status_with_context(
    validity: EvidenceValidity,
    quality: QualityGrade,
    risk: &AttributionRisk,
    concurrent_risk: bool,
) -> EvidenceStatus {
    let mut status = EvidenceStatus {
        validity,
        quality,
        reason_codes: Vec::new(),
    };
    if concurrent_risk || matches!(risk, AttributionRisk::KnownLocalOverlap { .. }) {
        status.quality = status.quality.max(QualityGrade::C);
        status
            .reason_codes
            .push(ReasonCode::ConcurrentUsagePossible);
    }
    status
}

fn isolation_quality(isolation: IsolationEvidence, risk: &AttributionRisk) -> QualityGrade {
    let explicit = match isolation {
        IsolationEvidence::ControlledBenchmark => QualityGrade::A,
        IsolationEvidence::IsolatedNormalTask => QualityGrade::B,
        IsolationEvidence::PossibleConcurrentUsage | IsolationEvidence::UnknownExternalUsage => {
            QualityGrade::C
        }
    };
    if matches!(risk, AttributionRisk::KnownLocalOverlap { .. }) {
        explicit.max(QualityGrade::C)
    } else {
        explicit
    }
}

fn lifecycle_for(
    reconciliation: &TaskQuotaReconciliation,
    token: &EvidenceStatus,
    five: &EvidenceStatus,
    weekly: &EvidenceStatus,
) -> ObservationLifecycle {
    let states = [
        &reconciliation.five_hour.state,
        &reconciliation.weekly.state,
    ];
    if states
        .iter()
        .any(|state| matches!(state, MeterReconciliationState::Reconciling { .. }))
    {
        return ObservationLifecycle::Reconciling;
    }
    if states
        .iter()
        .any(|state| matches!(state, MeterReconciliationState::AwaitingAfterSample))
    {
        return ObservationLifecycle::AwaitingMeter;
    }
    if token.validity != EvidenceValidity::Valid
        && five.validity == EvidenceValidity::Invalid
        && weekly.validity == EvidenceValidity::Invalid
    {
        ObservationLifecycle::Invalid
    } else if token.validity != EvidenceValidity::Valid
        || five.validity != EvidenceValidity::Valid
        || weekly.validity != EvidenceValidity::Valid
    {
        ObservationLifecycle::Incomplete
    } else {
        ObservationLifecycle::Finalized
    }
}

fn degrade_status(status: &mut EvidenceStatus, reason: ReasonCode) {
    if status.validity == EvidenceValidity::Valid {
        status.validity = EvidenceValidity::Incomplete;
        status.quality = status.quality.max(QualityGrade::D);
    }
    if !status.reason_codes.contains(&reason) {
        status.reason_codes.push(reason);
        status.reason_codes = sorted_reasons(std::mem::take(&mut status.reason_codes));
    }
}

fn worst_quality<const N: usize>(qualities: [QualityGrade; N]) -> QualityGrade {
    qualities.into_iter().max().expect("at least one quality")
}
fn sorted_reasons(mut reasons: Vec<ReasonCode>) -> Vec<ReasonCode> {
    reasons.sort();
    reasons.dedup();
    reasons
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry::normalized::{
        NormalizedEventType, NormalizedTokenPayload, SchemaVersion, TokenPayloadKind,
    };

    fn configuration(model: &str) -> ConfigurationIdentity {
        ConfigurationIdentity {
            plan: ConfigurationValue::unavailable(),
            model: ConfigurationValue::observed(model),
            reasoning_level: ConfigurationValue::observed("high"),
            speed_mode: ConfigurationValue::unavailable(),
            codex_version: ConfigurationValue::unavailable(),
        }
    }

    fn token_event(
        event_id: &str,
        source_instance_id: &str,
        task_id: &str,
        raw_total: Option<u64>,
        reasoning_output: Option<u64>,
        model: &str,
    ) -> AttributedTokenEvent {
        AttributedTokenEvent {
            normalized_event: super::super::normalized::NormalizedTokenEvent {
                schema_version: SchemaVersion::V1,
                event_id: event_id.to_owned(),
                event_type: NormalizedEventType::TokenCountersUpdated,
                source_instance_id: source_instance_id.to_owned(),
                safe_cursor_id: None,
                event_at: "2026-10-04T10:01:00Z".to_owned(),
                session_id: Some("session:test".to_owned()),
                task_id: Some(task_id.to_owned()),
                payload: NormalizedTokenPayload {
                    kind: TokenPayloadKind::TokenCounters,
                    token_counters: TokenCounters {
                        uncached_input: TokenMetric::unavailable(),
                        cached_input: TokenMetric::observed(10),
                        output: TokenMetric::observed(20),
                        reasoning_output: reasoning_output
                            .map_or_else(TokenMetric::unavailable, TokenMetric::observed),
                        raw_total: raw_total
                            .map_or_else(TokenMetric::unavailable, TokenMetric::observed),
                    },
                },
            },
            effective_configuration: configuration(model),
            configuration_fingerprint: format!("cfg:{model}"),
            task_lifecycle: None,
        }
    }

    #[test]
    fn reset_delta_is_rejected_by_serializer() {
        let evidence = ObservationQuotaEvidence {
            meter_type: QuotaMeterType::FiveHour,
            status: EvidenceStatus::new(
                EvidenceValidity::Invalid,
                QualityGrade::X,
                [ReasonCode::QuotaResetCrossed],
            ),
            before_sample: None,
            after_sample: None,
            delta_percentage_points: Some(1.0),
            window_identity: QuotaWindowIdentity::Unavailable,
            reset_status: ResetStatus::Detected,
        };
        assert!(serde_json::to_value(evidence).is_err());
    }

    #[test]
    fn quality_order_is_worst_grade() {
        assert_eq!(
            worst_quality([QualityGrade::B, QualityGrade::X, QualityGrade::B]),
            QualityGrade::X
        );
    }

    #[test]
    fn observation_id_is_replay_stable() {
        let first = identity::observation_id(
            "task:one",
            Some("session:one"),
            Some("2026-10-01T00:00:00Z"),
        );
        let second = identity::observation_id(
            "task:one",
            Some("session:one"),
            Some("2026-10-01T00:00:00Z"),
        );
        assert_eq!(first, second);
        assert!(first.starts_with("obs:"));
    }

    #[test]
    fn schema_version_remains_v1() {
        assert_eq!(serde_json::to_value(SchemaVersion::V1).unwrap(), "1.0.0");
    }

    #[test]
    fn token_aggregation_deduplicates_and_sums_raw_totals() {
        let first = token_event(
            "evt:1",
            "src:test",
            "task:test",
            Some(100),
            Some(3),
            "model-a",
        );
        let second = token_event(
            "evt:2",
            "src:test",
            "task:test",
            Some(250),
            Some(4),
            "model-a",
        );
        let duplicate = first.clone();
        let inputs = [first, second, duplicate];
        let (events, source) = deduplicate_events("task:test", &inputs).unwrap();
        let evidence = aggregate_tokens(
            &events,
            TokenTelemetryCompleteness::Complete,
            false,
            source.is_none(),
        )
        .unwrap();
        assert_eq!(evidence.status.validity, EvidenceValidity::Valid);
        assert_eq!(evidence.raw_token_counters.raw_total.value, Some(350));
        assert_eq!(evidence.raw_token_counters.reasoning_output.value, Some(7));
        assert_eq!(source.as_deref(), Some("src:test"));
    }

    #[test]
    fn partial_metric_is_unavailable_without_partial_sum() {
        let first = token_event(
            "evt:1",
            "src:test",
            "task:test",
            Some(100),
            Some(3),
            "model-a",
        );
        let second = token_event("evt:2", "src:test", "task:test", Some(250), None, "model-a");
        let inputs = [first, second];
        let (events, source) = deduplicate_events("task:test", &inputs).unwrap();
        let evidence = aggregate_tokens(
            &events,
            TokenTelemetryCompleteness::Complete,
            false,
            source.is_none(),
        )
        .unwrap();
        assert_eq!(
            evidence.raw_token_counters.reasoning_output.availability,
            MetricAvailability::Unavailable
        );
        assert_eq!(evidence.raw_token_counters.raw_total.value, Some(350));
    }

    #[test]
    fn conflicting_replay_and_overflow_are_structural_errors() {
        let first = token_event(
            "evt:1",
            "src:test",
            "task:test",
            Some(100),
            Some(3),
            "model-a",
        );
        let mut conflicting = first.clone();
        conflicting
            .normalized_event
            .payload
            .token_counters
            .raw_total = TokenMetric::observed(101);
        assert_eq!(
            deduplicate_events("task:test", &[first, conflicting]),
            Err(ObservationError::ConflictingTokenEvent)
        );

        let left = token_event(
            "evt:left",
            "src:test",
            "task:test",
            Some(MAX_SAFE_INTEGER),
            Some(3),
            "model-a",
        );
        let right = token_event(
            "evt:right",
            "src:test",
            "task:test",
            Some(1),
            Some(3),
            "model-a",
        );
        let inputs = [left, right];
        let (events, source) = deduplicate_events("task:test", &inputs).unwrap();
        assert_eq!(
            aggregate_tokens(
                &events,
                TokenTelemetryCompleteness::Complete,
                false,
                source.is_none()
            ),
            Err(ObservationError::TokenCounterOverflow)
        );
    }

    #[test]
    fn mixed_configuration_degrades_token_evidence_and_source_conflicts_omit_source() {
        let first = token_event("evt:1", "src:a", "task:test", Some(100), Some(3), "model-a");
        let second = token_event("evt:2", "src:b", "task:test", Some(250), Some(4), "model-b");
        let inputs = [first, second];
        let (events, source) = deduplicate_events("task:test", &inputs).unwrap();
        let evidence = aggregate_tokens(
            &events,
            TokenTelemetryCompleteness::Complete,
            true,
            source.is_none(),
        )
        .unwrap();
        assert!(source.is_none());
        assert_eq!(evidence.status.validity, EvidenceValidity::Incomplete);
        assert_eq!(evidence.status.quality, QualityGrade::D);
        assert_eq!(
            evidence.status.reason_codes,
            vec![ReasonCode::TelemetryIncomplete]
        );
    }
}
