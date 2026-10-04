//! Deterministic task-to-quota reconciliation without provider or runtime coupling.

use std::collections::BTreeSet;
use std::fmt;

use time::{format_description::well_known::Rfc3339, Duration, OffsetDateTime};

use super::normalized::{NormalizedQuotaSample, QuotaMeterType, QuotaWindowIdentity};
use super::quota_tracking::{
    advance_meter, BoundaryEvidence, MeterInstabilityReason, MeterTrackingState,
    NonNegativePercentagePoints, QuotaTrackingError, QuotaTrackingOutcome, TrackedQuotaWindow,
};

/// The privacy-safe identity and lifecycle interval of one task.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskReconciliationTarget {
    pub task_id: String,
    pub session_id: Option<String>,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub known_overlapping_task_ids: BTreeSet<String>,
}

impl TaskReconciliationTarget {
    pub fn new(
        task_id: impl Into<String>,
        started_at: Option<String>,
        ended_at: Option<String>,
    ) -> Self {
        Self {
            task_id: task_id.into(),
            session_id: None,
            started_at,
            ended_at,
            known_overlapping_task_ids: BTreeSet::new(),
        }
    }
}

/// Configurable, validated sampling and stabilization policy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReconciliationPolicy {
    pub max_before_sample_age: Duration,
    pub post_task_sample_offsets: Vec<Duration>,
    pub stabilization_not_before: Duration,
    pub required_stable_confirmations: usize,
    pub deadline: Duration,
}

impl ReconciliationPolicy {
    pub fn validate(&self) -> Result<(), ReconciliationError> {
        if self.max_before_sample_age.is_negative() {
            return Err(ReconciliationError::InvalidPolicy(
                PolicyValidationError::NegativeMaxBeforeSampleAge,
            ));
        }
        if self.post_task_sample_offsets.is_empty() {
            return Err(ReconciliationError::InvalidPolicy(
                PolicyValidationError::EmptySampleSchedule,
            ));
        }
        if self
            .post_task_sample_offsets
            .iter()
            .any(|offset| offset.is_negative())
        {
            return Err(ReconciliationError::InvalidPolicy(
                PolicyValidationError::NegativeSampleOffset,
            ));
        }
        if self
            .post_task_sample_offsets
            .windows(2)
            .any(|offsets| offsets[1] <= offsets[0])
        {
            return Err(ReconciliationError::InvalidPolicy(
                PolicyValidationError::NonIncreasingSampleOffsets,
            ));
        }
        if self.deadline.is_negative() {
            return Err(ReconciliationError::InvalidPolicy(
                PolicyValidationError::NegativeDeadline,
            ));
        }
        if self.stabilization_not_before.is_negative() {
            return Err(ReconciliationError::InvalidPolicy(
                PolicyValidationError::NegativeStabilizationThreshold,
            ));
        }
        if self.required_stable_confirmations == 0 {
            return Err(ReconciliationError::InvalidPolicy(
                PolicyValidationError::ZeroStableConfirmations,
            ));
        }
        let last_offset = *self
            .post_task_sample_offsets
            .last()
            .expect("non-empty schedule");
        if self.deadline < last_offset {
            return Err(ReconciliationError::InvalidPolicy(
                PolicyValidationError::DeadlineBeforeLastSample,
            ));
        }
        if self.stabilization_not_before > self.deadline {
            return Err(ReconciliationError::InvalidPolicy(
                PolicyValidationError::StabilizationAfterDeadline,
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PolicyValidationError {
    EmptySampleSchedule,
    NegativeMaxBeforeSampleAge,
    NegativeSampleOffset,
    NonIncreasingSampleOffsets,
    NegativeStabilizationThreshold,
    ZeroStableConfirmations,
    NegativeDeadline,
    DeadlineBeforeLastSample,
    StabilizationAfterDeadline,
}

impl fmt::Display for PolicyValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::EmptySampleSchedule => "post-task sample schedule cannot be empty",
            Self::NegativeMaxBeforeSampleAge => "maximum baseline age cannot be negative",
            Self::NegativeSampleOffset => "post-task sample offsets cannot be negative",
            Self::NonIncreasingSampleOffsets => {
                "post-task sample offsets must be strictly increasing"
            }
            Self::NegativeStabilizationThreshold => "stabilization threshold cannot be negative",
            Self::ZeroStableConfirmations => "stable confirmations must be greater than zero",
            Self::NegativeDeadline => "reconciliation deadline cannot be negative",
            Self::DeadlineBeforeLastSample => "deadline cannot precede the last allowed sample",
            Self::StabilizationAfterDeadline => "stabilization threshold cannot exceed deadline",
        };
        formatter.write_str(message)
    }
}

/// Explicit result of selecting a pre-task baseline.
#[derive(Clone, Debug, PartialEq)]
pub enum BeforeSampleSelection {
    Selected {
        sample: NormalizedQuotaSample,
        age: Duration,
    },
    Unavailable,
    TooOld {
        sample: NormalizedQuotaSample,
        age: Duration,
    },
    MissingTaskStart,
}

/// Why a meter has no usable task baseline.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BaselineUnavailableReason {
    MissingTaskStart,
    NoSampleBeforeTaskStart,
    SampleOlderThanMaximumAge,
}

/// Stable, provider-independent attribution-risk evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AttributionRisk {
    NoKnownLocalOverlap,
    KnownLocalOverlap { task_ids: BTreeSet<String> },
}

/// Acquisition failure reasons intentionally contain no provider error body.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum AcquisitionFailureReason {
    Temporary,
    SourceUnavailable,
    InvalidNormalizedEvidence,
    Unknown,
}

/// One scheduled acquisition opportunity, successful or failed.
#[derive(Clone, Debug, PartialEq)]
pub struct QuotaAcquisitionAttempt {
    pub sampled_at: String,
    pub meter_type: QuotaMeterType,
    pub result: QuotaAcquisitionResult,
}

pub type ReconciliationInput = QuotaAcquisitionAttempt;

#[derive(Clone, Debug, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum QuotaAcquisitionResult {
    Sample(NormalizedQuotaSample),
    Failed { reason: AcquisitionFailureReason },
}

/// State machine for one independent meter.
#[derive(Clone, Debug, PartialEq)]
pub enum MeterReconciliationState {
    NoBaseline { reason: BaselineUnavailableReason },
    AwaitingAfterSample,
    Reconciling { candidate: Option<StableCandidate> },
    Stable { evidence: ReconciledMeterEvidence },
    ResetCrossed { evidence: ResetCrossedEvidence },
    MeterUnstable { evidence: MeterUnstableEvidence },
    PlanDiscontinuity { evidence: PlanDiscontinuityEvidence },
    TimedOut { evidence: TimedOutEvidence },
    AcquisitionFailed { evidence: TimedOutEvidence },
}

/// Candidate semantics count the first eligible observation as confirmation one.
#[derive(Clone, Debug, PartialEq)]
pub struct StableCandidate {
    pub sample: NormalizedQuotaSample,
    pub window: TrackedQuotaWindow,
    pub consecutive_confirmations: usize,
    pub first_observed_at: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ReconciledMeterEvidence {
    pub meter_type: QuotaMeterType,
    pub before_sample: NormalizedQuotaSample,
    pub after_sample: NormalizedQuotaSample,
    pub delta_percentage_points: NonNegativePercentagePoints,
    pub window_identity: QuotaWindowIdentity,
    pub stabilized_at: String,
    pub acquisition_attempt_count: usize,
    pub last_change_at: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResetCrossedEvidence {
    pub meter_type: QuotaMeterType,
    pub before_sample: NormalizedQuotaSample,
    pub boundary_sample: NormalizedQuotaSample,
    pub previous_window: TrackedQuotaWindow,
    pub new_window: TrackedQuotaWindow,
    pub evidence: BoundaryEvidence,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MeterUnstableEvidence {
    pub meter_type: QuotaMeterType,
    pub before_sample: NormalizedQuotaSample,
    pub relevant_sample: NormalizedQuotaSample,
    pub window: TrackedQuotaWindow,
    pub reason: MeterInstabilityReason,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlanDiscontinuityEvidence {
    pub meter_type: QuotaMeterType,
    pub before_sample: NormalizedQuotaSample,
    pub relevant_sample: NormalizedQuotaSample,
    pub window: TrackedQuotaWindow,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TimedOutEvidence {
    pub meter_type: QuotaMeterType,
    pub before_sample: Option<NormalizedQuotaSample>,
    pub latest_candidate: Option<NormalizedQuotaSample>,
    pub last_successful_sample: Option<NormalizedQuotaSample>,
    pub deadline: String,
    pub acquisition_attempt_count: usize,
}

/// Explicit per-meter state. The tracker is private so P011 remains the only
/// implementation of reset-safe comparison semantics.
#[derive(Clone, Debug, PartialEq)]
pub struct MeterReconciliation {
    pub meter_type: QuotaMeterType,
    pub before_selection: BeforeSampleSelection,
    pub before_sample: Option<NormalizedQuotaSample>,
    pub state: MeterReconciliationState,
    pub acquisition_attempt_count: usize,
    pub last_successful_sample: Option<NormalizedQuotaSample>,
    pub last_change_at: Option<String>,
    tracker: MeterTrackingState,
    accumulated_delta: Option<NonNegativePercentagePoints>,
    seen_attempt_ids: BTreeSet<String>,
    last_attempt_at: Option<String>,
}

impl Eq for MeterReconciliation {}

impl MeterReconciliation {
    fn new(
        meter_type: QuotaMeterType,
        selection: BeforeSampleSelection,
    ) -> Result<Self, ReconciliationError> {
        let (before_sample, state, tracker) = match &selection {
            BeforeSampleSelection::Selected { sample, .. } => {
                let mut tracker = MeterTrackingState::new(meter_type);
                advance_meter(&mut tracker, sample).map_err(ReconciliationError::Tracking)?;
                (
                    Some(sample.clone()),
                    MeterReconciliationState::AwaitingAfterSample,
                    tracker,
                )
            }
            BeforeSampleSelection::Unavailable | BeforeSampleSelection::MissingTaskStart => (
                None,
                MeterReconciliationState::NoBaseline {
                    reason: BaselineUnavailableReason::MissingTaskStart,
                },
                MeterTrackingState::new(meter_type),
            ),
            BeforeSampleSelection::TooOld { .. } => (
                None,
                MeterReconciliationState::NoBaseline {
                    reason: BaselineUnavailableReason::SampleOlderThanMaximumAge,
                },
                MeterTrackingState::new(meter_type),
            ),
        };
        let state = match (&selection, state) {
            (BeforeSampleSelection::Unavailable, _) => MeterReconciliationState::NoBaseline {
                reason: BaselineUnavailableReason::NoSampleBeforeTaskStart,
            },
            (BeforeSampleSelection::MissingTaskStart, _) => MeterReconciliationState::NoBaseline {
                reason: BaselineUnavailableReason::MissingTaskStart,
            },
            (_, state) => state,
        };
        Ok(Self {
            meter_type,
            before_selection: selection,
            before_sample,
            state,
            acquisition_attempt_count: 0,
            last_successful_sample: None,
            last_change_at: None,
            tracker,
            accumulated_delta: None,
            seen_attempt_ids: BTreeSet::new(),
            last_attempt_at: None,
        })
    }

    fn is_terminal(&self) -> bool {
        matches!(
            self.state,
            MeterReconciliationState::NoBaseline { .. }
                | MeterReconciliationState::Stable { .. }
                | MeterReconciliationState::ResetCrossed { .. }
                | MeterReconciliationState::MeterUnstable { .. }
                | MeterReconciliationState::PlanDiscontinuity { .. }
                | MeterReconciliationState::TimedOut { .. }
                | MeterReconciliationState::AcquisitionFailed { .. }
        )
    }
}

/// Independent five-hour and weekly task evidence.
#[derive(Clone, Debug, PartialEq)]
pub struct TaskQuotaReconciliation {
    pub task: TaskReconciliationTarget,
    pub five_hour: MeterReconciliation,
    pub weekly: MeterReconciliation,
    pub attribution_risk: AttributionRisk,
}

impl Eq for TaskQuotaReconciliation {}

#[derive(Clone, Debug, PartialEq)]
pub struct ReconciliationBatch {
    pub outputs: Vec<ReconciliationOutput>,
    pub actions: Vec<ReconciliationAction>,
    pub next_state: TaskQuotaReconciliation,
}

#[derive(Clone, Debug, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum ReconciliationOutput {
    SampleAccepted {
        meter_type: QuotaMeterType,
        sample: NormalizedQuotaSample,
    },
    AcquisitionFailed {
        meter_type: QuotaMeterType,
        reason: AcquisitionFailureReason,
    },
    MeterTerminal {
        meter_type: QuotaMeterType,
        state: MeterReconciliationState,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReconciliationAction {
    AwaitSample {
        meter_type: QuotaMeterType,
        due_at: String,
    },
    RequestAnotherSample {
        meter_type: QuotaMeterType,
        not_before: String,
    },
    AwaitDeadline {
        meter_type: QuotaMeterType,
        at: String,
    },
    ReconciliationComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReconciliationError {
    InvalidPolicy(PolicyValidationError),
    MissingTaskEnd,
    TaskEndedBeforeStart,
    InvalidTaskTimestamp,
    InvalidInputTimestamp,
    AttemptSampleTimestampMismatch,
    AttemptMeterMismatch,
    TimestampOverflow,
    Tracking(QuotaTrackingError),
}

impl fmt::Display for ReconciliationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPolicy(error) => {
                write!(formatter, "invalid reconciliation policy: {error}")
            }
            Self::MissingTaskEnd => {
                formatter.write_str("task end timestamp is required for reconciliation")
            }
            Self::TaskEndedBeforeStart => formatter.write_str("task ended before it started"),
            Self::InvalidTaskTimestamp => formatter.write_str("task timestamp is invalid"),
            Self::InvalidInputTimestamp => {
                formatter.write_str("quota acquisition timestamp is invalid")
            }
            Self::AttemptSampleTimestampMismatch => formatter
                .write_str("acquisition timestamp does not match normalized sample timestamp"),
            Self::AttemptMeterMismatch => {
                formatter.write_str("acquisition meter does not match normalized sample")
            }
            Self::TimestampOverflow => {
                formatter.write_str("reconciliation timestamp arithmetic overflowed")
            }
            Self::Tracking(error) => write!(formatter, "quota tracking error: {error}"),
        }
    }
}

impl std::error::Error for ReconciliationError {}

/// Selects the latest eligible sample at or before task start, with age validation.
pub fn select_before_sample(
    meter_type: QuotaMeterType,
    task_started_at: Option<&str>,
    candidate_samples: &[NormalizedQuotaSample],
    policy: &ReconciliationPolicy,
) -> Result<BeforeSampleSelection, ReconciliationError> {
    policy.validate()?;
    let Some(task_started_at) = task_started_at else {
        return Ok(BeforeSampleSelection::MissingTaskStart);
    };
    let task_start = parse_timestamp(task_started_at, true)?;
    let mut eligible = Vec::new();
    for sample in candidate_samples {
        if sample.meter_type != meter_type {
            continue;
        }
        let sampled_at = parse_timestamp(&sample.sampled_at, false)?;
        if sampled_at <= task_start {
            eligible.push((sampled_at, sample));
        }
    }
    eligible.sort_by(|(left_time, left), (right_time, right)| {
        left_time
            .cmp(right_time)
            .then_with(|| left.sample_id.cmp(&right.sample_id))
    });
    let Some((sampled_at, sample)) = eligible.pop() else {
        return Ok(BeforeSampleSelection::Unavailable);
    };
    let age = task_start - sampled_at;
    if age > policy.max_before_sample_age {
        return Ok(BeforeSampleSelection::TooOld {
            sample: sample.clone(),
            age,
        });
    }
    Ok(BeforeSampleSelection::Selected {
        sample: sample.clone(),
        age,
    })
}

/// Initializes both independent meter reconciliations from historical evidence.
pub fn begin_task_quota_reconciliation(
    task: TaskReconciliationTarget,
    historical_samples: &[NormalizedQuotaSample],
    policy: &ReconciliationPolicy,
) -> Result<TaskQuotaReconciliation, ReconciliationError> {
    policy.validate()?;
    let ended_at = task
        .ended_at
        .as_deref()
        .ok_or(ReconciliationError::MissingTaskEnd)?;
    let ended = parse_timestamp(ended_at, true)?;
    if let Some(started_at) = task.started_at.as_deref() {
        if parse_timestamp(started_at, true)? > ended {
            return Err(ReconciliationError::TaskEndedBeforeStart);
        }
    }
    let five_selection = select_before_sample(
        QuotaMeterType::FiveHour,
        task.started_at.as_deref(),
        historical_samples,
        policy,
    )?;
    let weekly_selection = select_before_sample(
        QuotaMeterType::Weekly,
        task.started_at.as_deref(),
        historical_samples,
        policy,
    )?;
    let attribution_risk = if task.known_overlapping_task_ids.is_empty() {
        AttributionRisk::NoKnownLocalOverlap
    } else {
        AttributionRisk::KnownLocalOverlap {
            task_ids: task.known_overlapping_task_ids.clone(),
        }
    };
    let _ = ended;
    Ok(TaskQuotaReconciliation {
        task,
        five_hour: MeterReconciliation::new(QuotaMeterType::FiveHour, five_selection)?,
        weekly: MeterReconciliation::new(QuotaMeterType::Weekly, weekly_selection)?,
        attribution_risk,
    })
}

/// Replays inputs using their latest timestamp as the explicit evaluation time.
pub fn reconcile_task_quota(
    current: &TaskQuotaReconciliation,
    inputs: &[ReconciliationInput],
    policy: &ReconciliationPolicy,
) -> Result<ReconciliationBatch, ReconciliationError> {
    let evaluate_at = inputs
        .iter()
        .map(|input| parse_timestamp(&input.sampled_at, false))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .max()
        .map(format_timestamp)
        .transpose()?
        .unwrap_or_else(|| {
            current
                .task
                .ended_at
                .as_deref()
                .expect("validated task end")
                .to_owned()
        });
    reconcile_task_quota_at(current, inputs, policy, &evaluate_at)
}

/// Applies ordered evidence and evaluates deadline/actions at an explicit time.
pub fn reconcile_task_quota_at(
    current: &TaskQuotaReconciliation,
    inputs: &[ReconciliationInput],
    policy: &ReconciliationPolicy,
    evaluate_at: &str,
) -> Result<ReconciliationBatch, ReconciliationError> {
    policy.validate()?;
    let mut ordered = inputs.to_vec();
    validate_and_sort_inputs(&mut ordered)?;
    let evaluate_at = parse_timestamp(evaluate_at, true)?;
    let mut next_state = current.clone();
    let mut outputs = Vec::new();
    for input in ordered {
        process_attempt(&mut next_state, input, policy, evaluate_at, &mut outputs)?;
    }
    finalize_at_deadline(&mut next_state, policy, evaluate_at, &mut outputs)?;
    let actions = actions_for(&next_state, policy, evaluate_at)?;
    Ok(ReconciliationBatch {
        outputs,
        actions,
        next_state,
    })
}

fn validate_and_sort_inputs(inputs: &mut [ReconciliationInput]) -> Result<(), ReconciliationError> {
    for input in inputs.iter() {
        let attempt_time = parse_timestamp(&input.sampled_at, false)?;
        match &input.result {
            QuotaAcquisitionResult::Sample(sample) => {
                if sample.meter_type != input.meter_type {
                    return Err(ReconciliationError::AttemptMeterMismatch);
                }
                let sample_time = parse_timestamp(&sample.sampled_at, false)?;
                if sample_time != attempt_time {
                    return Err(ReconciliationError::AttemptSampleTimestampMismatch);
                }
            }
            QuotaAcquisitionResult::Failed { .. } => {}
        }
    }
    inputs.sort_by(|left, right| {
        let left_time = parse_timestamp(&left.sampled_at, false).expect("validated timestamp");
        let right_time = parse_timestamp(&right.sampled_at, false).expect("validated timestamp");
        left_time
            .cmp(&right_time)
            .then_with(|| meter_sort_key(left.meter_type).cmp(&meter_sort_key(right.meter_type)))
            .then_with(|| attempt_tie_key(left).cmp(&attempt_tie_key(right)))
    });
    Ok(())
}

fn attempt_tie_key(attempt: &QuotaAcquisitionAttempt) -> String {
    match &attempt.result {
        QuotaAcquisitionResult::Sample(sample) => format!("sample:{}", sample.sample_id),
        QuotaAcquisitionResult::Failed { reason } => format!("failure:{reason:?}"),
    }
}

fn meter_sort_key(meter_type: QuotaMeterType) -> u8 {
    match meter_type {
        QuotaMeterType::FiveHour => 0,
        QuotaMeterType::Weekly => 1,
    }
}

fn process_attempt(
    state: &mut TaskQuotaReconciliation,
    input: QuotaAcquisitionAttempt,
    policy: &ReconciliationPolicy,
    evaluate_at: OffsetDateTime,
    outputs: &mut Vec<ReconciliationOutput>,
) -> Result<(), ReconciliationError> {
    let meter = match input.meter_type {
        QuotaMeterType::FiveHour => &mut state.five_hour,
        QuotaMeterType::Weekly => &mut state.weekly,
    };
    if meter.is_terminal() || meter.before_sample.is_none() {
        return Ok(());
    }
    let attempt_time = parse_timestamp(&input.sampled_at, false)?;
    let deadline = deadline_for(&state.task, policy)?;
    if attempt_time > deadline || attempt_time > evaluate_at {
        return Ok(());
    }
    let attempt_id = format!(
        "{}:{}:{}",
        meter.meter_type.as_str(),
        input.sampled_at,
        attempt_tie_key(&input)
    );
    if !meter.seen_attempt_ids.insert(attempt_id) {
        return Ok(());
    }
    meter.acquisition_attempt_count += 1;
    meter.last_attempt_at = Some(input.sampled_at.clone());
    match input.result {
        QuotaAcquisitionResult::Failed { reason } => {
            meter.state = match &meter.state {
                MeterReconciliationState::AwaitingAfterSample => {
                    MeterReconciliationState::Reconciling { candidate: None }
                }
                state @ MeterReconciliationState::Reconciling { .. } => state.clone(),
                state => state.clone(),
            };
            outputs.push(ReconciliationOutput::AcquisitionFailed {
                meter_type: meter.meter_type,
                reason,
            });
        }
        QuotaAcquisitionResult::Sample(sample) => {
            let sample_time = parse_timestamp(&sample.sampled_at, false)?;
            let outcome = advance_meter(&mut meter.tracker, &sample)
                .map_err(ReconciliationError::Tracking)?;
            if !matches!(
                outcome,
                QuotaTrackingOutcome::DuplicateSample { .. }
                    | QuotaTrackingOutcome::OutOfOrderSample { .. }
            ) {
                let previous_successful_sample = meter
                    .last_successful_sample
                    .as_ref()
                    .or(meter.before_sample.as_ref())
                    .cloned();
                meter.last_successful_sample = Some(sample.clone());
                if let Some(previous) = &previous_successful_sample {
                    if used_percent(previous) != used_percent(&sample) {
                        meter.last_change_at = Some(sample.sampled_at.clone());
                    }
                }
                outputs.push(ReconciliationOutput::SampleAccepted {
                    meter_type: meter.meter_type,
                    sample: sample.clone(),
                });
            }
            handle_tracking_outcome(meter, outcome, sample_time, &state.task, policy, outputs)?;
        }
    }
    Ok(())
}

fn handle_tracking_outcome(
    meter: &mut MeterReconciliation,
    outcome: QuotaTrackingOutcome,
    sample_time: OffsetDateTime,
    task: &TaskReconciliationTarget,
    policy: &ReconciliationPolicy,
    outputs: &mut Vec<ReconciliationOutput>,
) -> Result<(), ReconciliationError> {
    match outcome {
        QuotaTrackingOutcome::SameWindowDelta(delta) => {
            let ended =
                parse_timestamp(task.ended_at.as_deref().expect("validated task end"), true)?;
            let threshold = ended
                .checked_add(policy.stabilization_not_before)
                .ok_or(ReconciliationError::TimestampOverflow)?;
            let accumulated = meter.accumulated_delta.map_or(0.0, |value| value.value())
                + delta.delta_percentage_points.value();
            meter.accumulated_delta = Some(
                NonNegativePercentagePoints::new(accumulated)
                    .map_err(ReconciliationError::Tracking)?,
            );
            if sample_time < ended {
                return Ok(());
            }
            meter.state = match &meter.state {
                MeterReconciliationState::AwaitingAfterSample => {
                    MeterReconciliationState::Reconciling { candidate: None }
                }
                state @ MeterReconciliationState::Reconciling { .. } => state.clone(),
                state => state.clone(),
            };
            if sample_time < threshold {
                return Ok(());
            }
            let first_observed_at = delta.after_sample.sampled_at.clone();
            let candidate = StableCandidate {
                sample: delta.after_sample,
                window: delta.window,
                consecutive_confirmations: 1,
                first_observed_at,
            };
            let existing = match &meter.state {
                MeterReconciliationState::Reconciling { candidate } => candidate.clone(),
                _ => None,
            };
            let candidate = match existing {
                Some(existing)
                    if same_window_identity(&existing.window, &candidate.window)
                        && used_percent(&existing.sample) == used_percent(&candidate.sample) =>
                {
                    StableCandidate {
                        sample: candidate.sample,
                        window: candidate.window,
                        consecutive_confirmations: existing.consecutive_confirmations + 1,
                        first_observed_at: existing.first_observed_at,
                    }
                }
                _ => candidate,
            };
            if candidate.consecutive_confirmations >= policy.required_stable_confirmations {
                let before_sample = meter.before_sample.clone().expect("selected baseline");
                let stabilized_at = candidate.sample.sampled_at.clone();
                let evidence = ReconciledMeterEvidence {
                    meter_type: meter.meter_type,
                    before_sample,
                    after_sample: candidate.sample,
                    delta_percentage_points: meter.accumulated_delta.expect("same-window delta"),
                    window_identity: candidate.window.quota_window_identity(),
                    stabilized_at,
                    acquisition_attempt_count: meter.acquisition_attempt_count,
                    last_change_at: meter.last_change_at.clone(),
                };
                meter.state = MeterReconciliationState::Stable { evidence };
                outputs.push(ReconciliationOutput::MeterTerminal {
                    meter_type: meter.meter_type,
                    state: meter.state.clone(),
                });
            } else {
                meter.state = MeterReconciliationState::Reconciling {
                    candidate: Some(candidate),
                };
            }
        }
        QuotaTrackingOutcome::DuplicateSample { .. }
        | QuotaTrackingOutcome::OutOfOrderSample { .. } => {}
        QuotaTrackingOutcome::BaselineEstablished { .. } => {}
        QuotaTrackingOutcome::ResetDetected {
            meter_type,
            before_sample,
            after_sample,
            previous_window,
            new_window,
            evidence,
        } => {
            meter.state = MeterReconciliationState::ResetCrossed {
                evidence: ResetCrossedEvidence {
                    meter_type,
                    before_sample,
                    boundary_sample: after_sample,
                    previous_window,
                    new_window,
                    evidence,
                },
            };
            outputs.push(ReconciliationOutput::MeterTerminal {
                meter_type: meter.meter_type,
                state: meter.state.clone(),
            });
        }
        QuotaTrackingOutcome::InferredWindowBoundary {
            meter_type,
            before_sample,
            after_sample,
            previous_window,
            new_window,
        } => {
            meter.state = MeterReconciliationState::ResetCrossed {
                evidence: ResetCrossedEvidence {
                    meter_type,
                    before_sample,
                    boundary_sample: after_sample,
                    previous_window,
                    new_window,
                    evidence: BoundaryEvidence::LocalInference,
                },
            };
            outputs.push(ReconciliationOutput::MeterTerminal {
                meter_type: meter.meter_type,
                state: meter.state.clone(),
            });
        }
        QuotaTrackingOutcome::MeterUnstable {
            meter_type,
            before_sample,
            after_sample,
            window,
            reason,
        } => {
            meter.state = MeterReconciliationState::MeterUnstable {
                evidence: MeterUnstableEvidence {
                    meter_type,
                    before_sample,
                    relevant_sample: after_sample,
                    window,
                    reason,
                },
            };
            outputs.push(ReconciliationOutput::MeterTerminal {
                meter_type: meter.meter_type,
                state: meter.state.clone(),
            });
        }
        QuotaTrackingOutcome::PlanDiscontinuity {
            meter_type,
            before_sample,
            after_sample,
            window,
        } => {
            meter.state = MeterReconciliationState::PlanDiscontinuity {
                evidence: PlanDiscontinuityEvidence {
                    meter_type,
                    before_sample,
                    relevant_sample: after_sample,
                    window,
                },
            };
            outputs.push(ReconciliationOutput::MeterTerminal {
                meter_type: meter.meter_type,
                state: meter.state.clone(),
            });
        }
    }
    Ok(())
}

fn finalize_at_deadline(
    state: &mut TaskQuotaReconciliation,
    policy: &ReconciliationPolicy,
    evaluate_at: OffsetDateTime,
    outputs: &mut Vec<ReconciliationOutput>,
) -> Result<(), ReconciliationError> {
    let deadline = deadline_for(&state.task, policy)?;
    if evaluate_at < deadline {
        return Ok(());
    }
    for meter in [&mut state.five_hour, &mut state.weekly] {
        if meter.before_sample.is_none() || meter.is_terminal() {
            continue;
        }
        let evidence = TimedOutEvidence {
            meter_type: meter.meter_type,
            before_sample: meter.before_sample.clone(),
            latest_candidate: match &meter.state {
                MeterReconciliationState::Reconciling { candidate } => {
                    candidate.as_ref().map(|c| c.sample.clone())
                }
                _ => None,
            },
            last_successful_sample: meter.last_successful_sample.clone(),
            deadline: format_timestamp(deadline)?,
            acquisition_attempt_count: meter.acquisition_attempt_count,
        };
        meter.state =
            if meter.acquisition_attempt_count > 0 && meter.last_successful_sample.is_none() {
                MeterReconciliationState::AcquisitionFailed { evidence }
            } else {
                MeterReconciliationState::TimedOut { evidence }
            };
        outputs.push(ReconciliationOutput::MeterTerminal {
            meter_type: meter.meter_type,
            state: meter.state.clone(),
        });
    }
    Ok(())
}

fn actions_for(
    state: &TaskQuotaReconciliation,
    policy: &ReconciliationPolicy,
    evaluate_at: OffsetDateTime,
) -> Result<Vec<ReconciliationAction>, ReconciliationError> {
    let mut actions = Vec::new();
    let deadline = deadline_for(&state.task, policy)?;
    for meter in [&state.five_hour, &state.weekly] {
        if meter.before_sample.is_none() || meter.is_terminal() {
            continue;
        }
        let next_due = next_due_at(meter, &state.task, policy)?;
        match next_due {
            Some(due) if due > evaluate_at => actions.push(ReconciliationAction::AwaitSample {
                meter_type: meter.meter_type,
                due_at: format_timestamp(due)?,
            }),
            Some(due) if due <= deadline => {
                actions.push(ReconciliationAction::RequestAnotherSample {
                    meter_type: meter.meter_type,
                    not_before: format_timestamp(evaluate_at.max(due))?,
                })
            }
            _ if evaluate_at < deadline => actions.push(ReconciliationAction::AwaitDeadline {
                meter_type: meter.meter_type,
                at: format_timestamp(deadline)?,
            }),
            _ => {}
        }
    }
    if state.five_hour.is_terminal() && state.weekly.is_terminal() {
        actions.push(ReconciliationAction::ReconciliationComplete);
    }
    Ok(actions)
}

fn next_due_at(
    meter: &MeterReconciliation,
    task: &TaskReconciliationTarget,
    policy: &ReconciliationPolicy,
) -> Result<Option<OffsetDateTime>, ReconciliationError> {
    let ended = parse_timestamp(task.ended_at.as_deref().expect("validated task end"), true)?;
    for offset in &policy.post_task_sample_offsets {
        let due = ended
            .checked_add(*offset)
            .ok_or(ReconciliationError::TimestampOverflow)?;
        if meter.last_attempt_at.as_deref().is_none_or(|last| {
            parse_timestamp(last, false).expect("validated attempt timestamp") < due
        }) {
            return Ok(Some(due));
        }
    }
    Ok(None)
}

fn deadline_for(
    task: &TaskReconciliationTarget,
    policy: &ReconciliationPolicy,
) -> Result<OffsetDateTime, ReconciliationError> {
    let ended = parse_timestamp(
        task.ended_at
            .as_deref()
            .ok_or(ReconciliationError::MissingTaskEnd)?,
        true,
    )?;
    ended
        .checked_add(policy.deadline)
        .ok_or(ReconciliationError::TimestampOverflow)
}

fn parse_timestamp(value: &str, task: bool) -> Result<OffsetDateTime, ReconciliationError> {
    OffsetDateTime::parse(value, &Rfc3339).map_err(|_| {
        if task {
            ReconciliationError::InvalidTaskTimestamp
        } else {
            ReconciliationError::InvalidInputTimestamp
        }
    })
}

fn format_timestamp(value: OffsetDateTime) -> Result<String, ReconciliationError> {
    value
        .format(&Rfc3339)
        .map_err(|_| ReconciliationError::InvalidInputTimestamp)
}

fn used_percent(sample: &NormalizedQuotaSample) -> f64 {
    sample.used_percent.value.unwrap_or(f64::NAN)
}

fn same_window_identity(left: &TrackedQuotaWindow, right: &TrackedQuotaWindow) -> bool {
    match (&left.identity, &right.identity) {
        (
            super::quota_tracking::TrackedWindowIdentity::ObservedReset {
                observed_reset_at: left,
            },
            super::quota_tracking::TrackedWindowIdentity::ObservedReset {
                observed_reset_at: right,
            },
        ) => left == right,
        (
            super::quota_tracking::TrackedWindowIdentity::LocallyInferred {
                local_window_id: left,
            },
            super::quota_tracking::TrackedWindowIdentity::LocallyInferred {
                local_window_id: right,
            },
        ) => left == right,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry::normalized::{
        AcquisitionStatus, ConfigurationIdentity, ConfigurationValue, MetricAvailability,
        MetricProvenance, PercentageMetric, QuotaIdentityProvenance, QuotaSourceKind,
        SchemaVersion,
    };

    fn policy() -> ReconciliationPolicy {
        ReconciliationPolicy {
            max_before_sample_age: Duration::hours(1),
            post_task_sample_offsets: vec![
                Duration::ZERO,
                Duration::seconds(15),
                Duration::seconds(30),
                Duration::seconds(60),
            ],
            stabilization_not_before: Duration::seconds(15),
            required_stable_confirmations: 2,
            deadline: Duration::seconds(60),
        }
    }

    fn sample(
        meter_type: QuotaMeterType,
        id: &str,
        at: &str,
        used: f64,
        reset: Option<&str>,
        plan: Option<&str>,
    ) -> NormalizedQuotaSample {
        NormalizedQuotaSample {
            schema_version: SchemaVersion::V1,
            sample_id: id.to_owned(),
            meter_type,
            sampled_at: at.to_owned(),
            used_percent: PercentageMetric {
                availability: MetricAvailability::Available,
                value: Some(used),
                provenance: MetricProvenance::Observed,
            },
            remaining_percent: PercentageMetric {
                availability: MetricAvailability::Available,
                value: Some(100.0 - used),
                provenance: MetricProvenance::Derived,
            },
            reset_evidence: reset.map_or(QuotaWindowIdentity::Unavailable, |at| {
                QuotaWindowIdentity::Available {
                    meter_type,
                    local_window_id: None,
                    observed_reset_at: Some(at.to_owned()),
                    evidence_first_sample_at: None,
                    evidence_last_sample_at: None,
                    identity_provenance: QuotaIdentityProvenance::ObservedReset,
                    identity_confidence: super::super::normalized::QuotaIdentityConfidence::High,
                }
            }),
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

    fn target() -> TaskReconciliationTarget {
        TaskReconciliationTarget::new(
            "task",
            Some("2026-10-04T10:00:00Z".to_owned()),
            Some("2026-10-04T10:05:00Z".to_owned()),
        )
    }

    fn attempt(sample: NormalizedQuotaSample) -> QuotaAcquisitionAttempt {
        QuotaAcquisitionAttempt {
            sampled_at: sample.sampled_at.clone(),
            meter_type: sample.meter_type,
            result: QuotaAcquisitionResult::Sample(sample),
        }
    }

    #[test]
    fn delayed_update_does_not_finalize_immediate_zero() {
        let baseline = sample(
            QuotaMeterType::FiveHour,
            "before",
            "2026-10-04T09:59:00Z",
            20.0,
            None,
            None,
        );
        let current = begin_task_quota_reconciliation(target(), &[baseline], &policy()).unwrap();
        let inputs = vec![
            attempt(sample(
                QuotaMeterType::FiveHour,
                "a",
                "2026-10-04T10:05:00Z",
                20.0,
                Some("2026-10-04T15:00:00Z"),
                None,
            )),
            attempt(sample(
                QuotaMeterType::FiveHour,
                "b",
                "2026-10-04T10:05:15Z",
                25.0,
                Some("2026-10-04T15:00:00Z"),
                None,
            )),
            attempt(sample(
                QuotaMeterType::FiveHour,
                "c",
                "2026-10-04T10:05:30Z",
                25.0,
                Some("2026-10-04T15:00:00Z"),
                None,
            )),
        ];
        let result = reconcile_task_quota(&current, &inputs, &policy()).unwrap();
        let MeterReconciliationState::Stable { evidence } = result.next_state.five_hour.state
        else {
            panic!("expected stable")
        };
        assert_eq!(evidence.after_sample.sample_id, "c");
        assert_eq!(evidence.delta_percentage_points.value(), 5.0);
    }

    #[test]
    fn unchanged_meter_can_stabilize_to_zero() {
        let baseline = sample(
            QuotaMeterType::FiveHour,
            "before",
            "2026-10-04T09:59:00Z",
            20.0,
            None,
            None,
        );
        let current = begin_task_quota_reconciliation(target(), &[baseline], &policy()).unwrap();
        let inputs = vec![
            attempt(sample(
                QuotaMeterType::FiveHour,
                "a",
                "2026-10-04T10:05:15Z",
                20.0,
                None,
                None,
            )),
            attempt(sample(
                QuotaMeterType::FiveHour,
                "b",
                "2026-10-04T10:05:30Z",
                20.0,
                None,
                None,
            )),
        ];
        let result = reconcile_task_quota(&current, &inputs, &policy()).unwrap();
        let MeterReconciliationState::Stable { evidence } = result.next_state.five_hour.state
        else {
            panic!("expected stable")
        };
        assert_eq!(evidence.delta_percentage_points.value(), 0.0);
    }

    #[test]
    fn reset_during_task_is_terminal() {
        let baseline = sample(
            QuotaMeterType::FiveHour,
            "before",
            "2026-10-04T09:59:00Z",
            95.0,
            Some("2026-10-04T10:00:00Z"),
            None,
        );
        let current = begin_task_quota_reconciliation(target(), &[baseline], &policy()).unwrap();
        let reset = sample(
            QuotaMeterType::FiveHour,
            "reset",
            "2026-10-04T10:03:00Z",
            3.0,
            Some("2026-10-04T10:00:00Z"),
            None,
        );
        let result = reconcile_task_quota(&current, &[attempt(reset)], &policy()).unwrap();
        assert!(matches!(
            result.next_state.five_hour.state,
            MeterReconciliationState::ResetCrossed { .. }
        ));
    }

    #[test]
    fn independent_weekly_evidence_survives_five_hour_reset() {
        let five = sample(
            QuotaMeterType::FiveHour,
            "five-before",
            "2026-10-04T09:59:00Z",
            95.0,
            Some("2026-10-04T10:00:00Z"),
            None,
        );
        let weekly = sample(
            QuotaMeterType::Weekly,
            "week-before",
            "2026-10-04T09:59:00Z",
            30.0,
            Some("2026-10-11T09:59:00Z"),
            None,
        );
        let current =
            begin_task_quota_reconciliation(target(), &[five, weekly], &policy()).unwrap();
        let inputs = vec![
            attempt(sample(
                QuotaMeterType::FiveHour,
                "five-reset",
                "2026-10-04T10:03:00Z",
                3.0,
                Some("2026-10-04T10:00:00Z"),
                None,
            )),
            attempt(sample(
                QuotaMeterType::Weekly,
                "week-a",
                "2026-10-04T10:05:15Z",
                32.0,
                Some("2026-10-11T09:59:00Z"),
                None,
            )),
            attempt(sample(
                QuotaMeterType::Weekly,
                "week-b",
                "2026-10-04T10:05:30Z",
                32.0,
                Some("2026-10-11T09:59:00Z"),
                None,
            )),
        ];
        let result = reconcile_task_quota(&current, &inputs, &policy()).unwrap();
        assert!(matches!(
            result.next_state.five_hour.state,
            MeterReconciliationState::ResetCrossed { .. }
        ));
        let MeterReconciliationState::Stable { evidence } = result.next_state.weekly.state else {
            panic!("expected weekly stable")
        };
        assert_eq!(evidence.delta_percentage_points.value(), 2.0);
    }

    #[test]
    fn stale_baseline_is_not_selected() {
        let mut stale_policy = policy();
        stale_policy.max_before_sample_age = Duration::minutes(30);
        let selection = select_before_sample(
            QuotaMeterType::FiveHour,
            Some("2026-10-04T12:00:00Z"),
            &[sample(
                QuotaMeterType::FiveHour,
                "old",
                "2026-10-04T11:00:00Z",
                20.0,
                None,
                None,
            )],
            &stale_policy,
        )
        .unwrap();
        assert!(matches!(selection, BeforeSampleSelection::TooOld { .. }));
    }

    #[test]
    fn failures_are_not_zero_and_can_recover() {
        let baseline = sample(
            QuotaMeterType::FiveHour,
            "before",
            "2026-10-04T09:59:00Z",
            20.0,
            None,
            None,
        );
        let current = begin_task_quota_reconciliation(target(), &[baseline], &policy()).unwrap();
        let inputs = vec![
            QuotaAcquisitionAttempt {
                sampled_at: "2026-10-04T10:05:00Z".to_owned(),
                meter_type: QuotaMeterType::FiveHour,
                result: QuotaAcquisitionResult::Failed {
                    reason: AcquisitionFailureReason::Temporary,
                },
            },
            attempt(sample(
                QuotaMeterType::FiveHour,
                "a",
                "2026-10-04T10:05:15Z",
                25.0,
                None,
                None,
            )),
            attempt(sample(
                QuotaMeterType::FiveHour,
                "b",
                "2026-10-04T10:05:30Z",
                25.0,
                None,
                None,
            )),
        ];
        let result = reconcile_task_quota(&current, &inputs, &policy()).unwrap();
        assert!(matches!(
            result.next_state.five_hour.state,
            MeterReconciliationState::Stable { .. }
        ));
        assert_eq!(result.next_state.five_hour.acquisition_attempt_count, 3);
    }

    #[test]
    fn plan_change_is_terminal() {
        let baseline = sample(
            QuotaMeterType::FiveHour,
            "before",
            "2026-10-04T09:59:00Z",
            20.0,
            None,
            Some("plus"),
        );
        let current = begin_task_quota_reconciliation(target(), &[baseline], &policy()).unwrap();
        let changed = sample(
            QuotaMeterType::FiveHour,
            "changed",
            "2026-10-04T10:05:15Z",
            25.0,
            None,
            Some("pro"),
        );
        let result = reconcile_task_quota(&current, &[attempt(changed)], &policy()).unwrap();
        assert!(matches!(
            result.next_state.five_hour.state,
            MeterReconciliationState::PlanDiscontinuity { .. }
        ));
    }

    #[test]
    fn reset_during_stabilization_cannot_be_rehabilitated() {
        let baseline = sample(
            QuotaMeterType::FiveHour,
            "before",
            "2026-10-04T09:59:00Z",
            20.0,
            None,
            None,
        );
        let current = begin_task_quota_reconciliation(target(), &[baseline], &policy()).unwrap();
        let inputs = vec![
            attempt(sample(
                QuotaMeterType::FiveHour,
                "candidate",
                "2026-10-04T10:05:15Z",
                25.0,
                None,
                None,
            )),
            attempt(sample(
                QuotaMeterType::FiveHour,
                "reset",
                "2026-10-04T10:05:20Z",
                3.0,
                Some("2026-10-04T10:05:20Z"),
                None,
            )),
            attempt(sample(
                QuotaMeterType::FiveHour,
                "later",
                "2026-10-04T10:05:30Z",
                4.0,
                Some("2026-10-04T10:05:20Z"),
                None,
            )),
        ];
        let result = reconcile_task_quota(&current, &inputs, &policy()).unwrap();
        assert!(matches!(
            result.next_state.five_hour.state,
            MeterReconciliationState::ResetCrossed { .. }
        ));
    }

    #[test]
    fn same_window_decrease_is_terminal_instability() {
        let baseline = sample(
            QuotaMeterType::FiveHour,
            "before",
            "2026-10-04T09:59:00Z",
            20.0,
            Some("2026-10-04T10:10:00Z"),
            None,
        );
        let current = begin_task_quota_reconciliation(target(), &[baseline], &policy()).unwrap();
        let inputs = vec![
            attempt(sample(
                QuotaMeterType::FiveHour,
                "a",
                "2026-10-04T10:05:15Z",
                25.0,
                Some("2026-10-04T10:10:00Z"),
                None,
            )),
            attempt(sample(
                QuotaMeterType::FiveHour,
                "b",
                "2026-10-04T10:05:30Z",
                19.0,
                Some("2026-10-04T10:10:00Z"),
                None,
            )),
        ];
        let result = reconcile_task_quota(&current, &inputs, &policy()).unwrap();
        assert!(matches!(
            result.next_state.five_hour.state,
            MeterReconciliationState::MeterUnstable { .. }
        ));
    }

    #[test]
    fn deadline_does_not_promote_an_unstable_candidate() {
        let baseline = sample(
            QuotaMeterType::FiveHour,
            "before",
            "2026-10-04T09:59:00Z",
            20.0,
            None,
            None,
        );
        let current = begin_task_quota_reconciliation(target(), &[baseline], &policy()).unwrap();
        let input = attempt(sample(
            QuotaMeterType::FiveHour,
            "candidate",
            "2026-10-04T10:05:15Z",
            25.0,
            None,
            None,
        ));
        let result =
            reconcile_task_quota_at(&current, &[input], &policy(), "2026-10-04T10:06:00Z").unwrap();
        assert!(matches!(
            result.next_state.five_hour.state,
            MeterReconciliationState::TimedOut { .. }
        ));
    }

    #[test]
    fn failure_through_deadline_is_not_zero() {
        let baseline = sample(
            QuotaMeterType::FiveHour,
            "before",
            "2026-10-04T09:59:00Z",
            20.0,
            None,
            None,
        );
        let current = begin_task_quota_reconciliation(target(), &[baseline], &policy()).unwrap();
        let failure = QuotaAcquisitionAttempt {
            sampled_at: "2026-10-04T10:05:00Z".to_owned(),
            meter_type: QuotaMeterType::FiveHour,
            result: QuotaAcquisitionResult::Failed {
                reason: AcquisitionFailureReason::SourceUnavailable,
            },
        };
        let result =
            reconcile_task_quota_at(&current, &[failure], &policy(), "2026-10-04T10:06:00Z")
                .unwrap();
        assert!(matches!(
            result.next_state.five_hour.state,
            MeterReconciliationState::AcquisitionFailed { .. }
        ));
    }

    #[test]
    fn baseline_selection_is_latest_and_inclusive_but_not_during_task() {
        let reconciliation_policy = policy();
        let first = sample(
            QuotaMeterType::FiveHour,
            "first",
            "2026-10-04T09:59:00Z",
            20.0,
            None,
            None,
        );
        let equal = sample(
            QuotaMeterType::FiveHour,
            "equal",
            "2026-10-04T10:00:00Z",
            21.0,
            None,
            None,
        );
        let during = sample(
            QuotaMeterType::FiveHour,
            "during",
            "2026-10-04T10:02:00Z",
            22.0,
            None,
            None,
        );
        let selection = select_before_sample(
            QuotaMeterType::FiveHour,
            Some("2026-10-04T10:00:00Z"),
            &[first, equal.clone(), during],
            &reconciliation_policy,
        )
        .unwrap();
        let BeforeSampleSelection::Selected { sample, .. } = selection else {
            panic!("expected selected baseline")
        };
        assert_eq!(sample.sample_id, equal.sample_id);
    }

    #[test]
    fn missing_start_is_explicit_and_missing_end_is_structural() {
        let reconciliation_policy = policy();
        let target_without_start =
            TaskReconciliationTarget::new("task", None, Some("2026-10-04T10:05:00Z".to_owned()));
        let state =
            begin_task_quota_reconciliation(target_without_start, &[], &reconciliation_policy)
                .unwrap();
        assert!(matches!(
            state.five_hour.state,
            MeterReconciliationState::NoBaseline {
                reason: BaselineUnavailableReason::MissingTaskStart
            }
        ));
        assert!(matches!(
            state.weekly.state,
            MeterReconciliationState::NoBaseline {
                reason: BaselineUnavailableReason::MissingTaskStart
            }
        ));
        let result = reconcile_task_quota(&state, &[], &reconciliation_policy).unwrap();
        assert_eq!(
            result.actions,
            vec![ReconciliationAction::ReconciliationComplete]
        );
        let target_without_end =
            TaskReconciliationTarget::new("task", Some("2026-10-04T10:00:00Z".to_owned()), None);
        assert_eq!(
            begin_task_quota_reconciliation(target_without_end, &[], &reconciliation_policy)
                .unwrap_err(),
            ReconciliationError::MissingTaskEnd
        );
    }

    #[test]
    fn no_baseline_and_stable_meter_complete_without_no_baseline_action() {
        let weekly_baseline = sample(
            QuotaMeterType::Weekly,
            "weekly-before",
            "2026-10-04T09:59:00Z",
            30.0,
            None,
            None,
        );
        let current =
            begin_task_quota_reconciliation(target(), &[weekly_baseline], &policy()).unwrap();
        let inputs = vec![
            attempt(sample(
                QuotaMeterType::Weekly,
                "weekly-a",
                "2026-10-04T10:05:15Z",
                32.0,
                None,
                None,
            )),
            attempt(sample(
                QuotaMeterType::Weekly,
                "weekly-b",
                "2026-10-04T10:05:30Z",
                32.0,
                None,
                None,
            )),
        ];
        let result = reconcile_task_quota(&current, &inputs, &policy()).unwrap();
        assert!(matches!(
            result.next_state.five_hour.state,
            MeterReconciliationState::NoBaseline {
                reason: BaselineUnavailableReason::NoSampleBeforeTaskStart
            }
        ));
        assert!(matches!(
            result.next_state.weekly.state,
            MeterReconciliationState::Stable { .. }
        ));
        assert_eq!(
            result.actions,
            vec![ReconciliationAction::ReconciliationComplete]
        );
    }

    #[test]
    fn no_baseline_does_not_block_reconciling_meter_or_request_sampling() {
        let weekly_baseline = sample(
            QuotaMeterType::Weekly,
            "weekly-before",
            "2026-10-04T09:59:00Z",
            30.0,
            None,
            None,
        );
        let current =
            begin_task_quota_reconciliation(target(), &[weekly_baseline], &policy()).unwrap();
        let result = reconcile_task_quota(
            &current,
            &[attempt(sample(
                QuotaMeterType::Weekly,
                "weekly-a",
                "2026-10-04T10:05:15Z",
                32.0,
                None,
                None,
            ))],
            &policy(),
        )
        .unwrap();
        assert!(matches!(
            result.next_state.five_hour.state,
            MeterReconciliationState::NoBaseline { .. }
        ));
        assert!(matches!(
            result.next_state.weekly.state,
            MeterReconciliationState::Reconciling { .. }
        ));
        assert!(!result
            .actions
            .contains(&ReconciliationAction::ReconciliationComplete));
        assert!(result.actions.iter().any(|action| matches!(
            action,
            ReconciliationAction::AwaitSample {
                meter_type: QuotaMeterType::Weekly,
                ..
            }
        )));
        assert!(!result.actions.iter().any(|action| matches!(
            action,
            ReconciliationAction::AwaitSample {
                meter_type: QuotaMeterType::FiveHour,
                ..
            } | ReconciliationAction::RequestAnotherSample {
                meter_type: QuotaMeterType::FiveHour,
                ..
            } | ReconciliationAction::AwaitDeadline {
                meter_type: QuotaMeterType::FiveHour,
                ..
            }
        )));
    }

    #[test]
    fn stale_baseline_is_resolved_without_post_task_polling() {
        let mut stale_policy = policy();
        stale_policy.max_before_sample_age = Duration::minutes(30);
        let old_five_hour = sample(
            QuotaMeterType::FiveHour,
            "old-five-hour",
            "2026-10-04T11:00:00Z",
            20.0,
            None,
            None,
        );
        let mut task = target();
        task.started_at = Some("2026-10-04T12:00:00Z".to_owned());
        task.ended_at = Some("2026-10-04T12:05:00Z".to_owned());
        let current =
            begin_task_quota_reconciliation(task, &[old_five_hour], &stale_policy).unwrap();
        assert!(matches!(
            current.five_hour.state,
            MeterReconciliationState::NoBaseline {
                reason: BaselineUnavailableReason::SampleOlderThanMaximumAge
            }
        ));
        let result = reconcile_task_quota(&current, &[], &stale_policy).unwrap();
        assert_eq!(
            result.actions,
            vec![ReconciliationAction::ReconciliationComplete]
        );
    }

    #[test]
    fn policy_validation_is_deterministic() {
        let mut invalid = policy();
        invalid.post_task_sample_offsets = vec![Duration::seconds(15), Duration::seconds(15)];
        assert_eq!(
            invalid.validate().unwrap_err(),
            ReconciliationError::InvalidPolicy(PolicyValidationError::NonIncreasingSampleOffsets)
        );
        invalid = policy();
        invalid.required_stable_confirmations = 0;
        assert_eq!(
            invalid.validate().unwrap_err(),
            ReconciliationError::InvalidPolicy(PolicyValidationError::ZeroStableConfirmations)
        );
    }

    #[test]
    fn replay_and_overlap_evidence_are_stable() {
        let baseline = sample(
            QuotaMeterType::FiveHour,
            "before",
            "2026-10-04T09:59:00Z",
            20.0,
            None,
            None,
        );
        let mut task = target();
        task.known_overlapping_task_ids
            .insert("other-task".to_owned());
        let current = begin_task_quota_reconciliation(task, &[baseline], &policy()).unwrap();
        let inputs = vec![
            attempt(sample(
                QuotaMeterType::FiveHour,
                "a",
                "2026-10-04T10:05:15Z",
                25.0,
                None,
                None,
            )),
            attempt(sample(
                QuotaMeterType::FiveHour,
                "b",
                "2026-10-04T10:05:30Z",
                25.0,
                None,
                None,
            )),
        ];
        let first = reconcile_task_quota(&current, &inputs, &policy()).unwrap();
        let reversed = vec![inputs[1].clone(), inputs[0].clone()];
        let second = reconcile_task_quota(&current, &reversed, &policy()).unwrap();
        assert_eq!(first.next_state, second.next_state);
        assert_eq!(
            first.next_state.attribution_risk,
            AttributionRisk::KnownLocalOverlap {
                task_ids: ["other-task".to_owned()].into_iter().collect()
            }
        );
    }
}
