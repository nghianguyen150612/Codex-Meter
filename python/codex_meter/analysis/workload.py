"""Pure remaining-workload estimates from historical raw-token task sizes."""

from __future__ import annotations

import math
from dataclasses import dataclass, fields
from enum import StrEnum
from typing import TypeAlias

from codex_meter.estimation import (
    ESTIMATOR_METHOD_VERSION,
    PROVIDER_AUTHORITY,
    AnalyticsResultStatus,
    AnalyticsResultV1,
    Confidence,
    QuotaCapacityEstimate,
    ReasonCode,
    ResultConfigurationIdentity,
    ResultConfigurationValue,
)
from codex_meter.history import (
    ConfigurationKey,
    EvidenceValidity,
    ObservationDataset,
    ObservationLifecycle,
    ObservationRecord,
    QualityGrade,
    QuotaMeterType,
)

WORKLOAD_METHOD_VERSION = "1.0.0"
MINIMUM_TASK_SIZE_SAMPLES = 3
MAXIMUM_SAFE_TASK_COUNT = 9_007_199_254_740_991
# Mirrors P017's MAXIMUM_OBSERVATIONS_SAFETY_LIMIT without coupling analysis
# to the SQLite reader implementation.
MAXIMUM_WORKLOAD_OBSERVATIONS = 100_000


class WorkloadAnalysisError(ValueError):
    """Base error for invalid inputs or impossible workload calculations."""


class WorkloadDatasetError(WorkloadAnalysisError):
    """Invalid, nonterminal, duplicate, or oversized task history input."""


class WorkloadConfigurationMismatchError(WorkloadAnalysisError):
    """Task history and current capacity do not share exact configuration."""


class WorkloadCapacityError(WorkloadAnalysisError):
    """The supplied P020 capacity result is invalid for workload prediction."""


class WorkloadNumericalError(WorkloadAnalysisError):
    """A quantile or task-count calculation is not safely representable."""


class TaskSizeExclusionReason(StrEnum):
    """Why one terminal Observation did not enter the task-size distribution."""

    TOKEN_NOT_VALID = "token_not_valid"
    TOKEN_QUALITY_INELIGIBLE = "token_quality_ineligible"
    RAW_TOTAL_UNAVAILABLE = "raw_total_unavailable"
    ZERO_RAW_TOTAL = "zero_raw_total"


_TASK_SIZE_REASON_ORDER = tuple(TaskSizeExclusionReason)
_REASON_CODE_ORDER = tuple(ReasonCode)
_ELIGIBLE_TOKEN_QUALITIES = frozenset({QualityGrade.A, QualityGrade.B, QualityGrade.C})
_SUFFICIENT_CONFIDENCE = frozenset({Confidence.LOW, Confidence.MEDIUM, Confidence.HIGH})


def _validate_configuration(configuration: object) -> None:
    if not isinstance(configuration, ConfigurationKey) or any(
        value is not None and (not isinstance(value, str) or not value)
        for field in fields(ConfigurationKey)
        for value in (getattr(configuration, field.name),)
    ):
        raise WorkloadDatasetError("configuration must use five exact typed values")


def _finite_float(value: object, label: str, *, nonnegative: bool = False) -> float:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise WorkloadNumericalError(f"{label} must be a finite numeric value")
    try:
        result = float(value)
    except (OverflowError, ValueError) as exc:
        raise WorkloadNumericalError(f"{label} is outside the supported numeric range") from exc
    if not math.isfinite(result) or (nonnegative and result < 0):
        qualifier = "finite and non-negative" if nonnegative else "finite"
        raise WorkloadNumericalError(f"{label} must be {qualifier}")
    return 0.0 if result == 0.0 else result


@dataclass(frozen=True, slots=True)
class TaskSizeSample:
    """One eligible task's unchanged positive raw-token total."""

    observation_id: str
    configuration: ConfigurationKey
    raw_total: int
    token_quality: QualityGrade

    def __post_init__(self) -> None:
        if not isinstance(self.observation_id, str) or not self.observation_id:
            raise WorkloadDatasetError("task-size sample ID must be non-empty")
        _validate_configuration(self.configuration)
        if (
            isinstance(self.raw_total, bool)
            or not isinstance(self.raw_total, int)
            or self.raw_total <= 0
        ):
            raise WorkloadDatasetError("task-size samples require a positive integer raw total")
        if (
            not isinstance(self.token_quality, QualityGrade)
            or self.token_quality not in _ELIGIBLE_TOKEN_QUALITIES
        ):
            raise WorkloadDatasetError("task-size sample quality must be A, B, or C")


@dataclass(frozen=True, slots=True)
class TaskSizeExclusion:
    """One excluded terminal Observation and every applicable token reason."""

    observation_id: str
    reasons: tuple[TaskSizeExclusionReason, ...]

    def __post_init__(self) -> None:
        if not isinstance(self.observation_id, str) or not self.observation_id:
            raise WorkloadDatasetError("task-size exclusion ID must be non-empty")
        if (
            not isinstance(self.reasons, tuple)
            or not self.reasons
            or any(not isinstance(reason, TaskSizeExclusionReason) for reason in self.reasons)
            or self.reasons
            != tuple(reason for reason in _TASK_SIZE_REASON_ORDER if reason in self.reasons)
        ):
            raise WorkloadDatasetError("task-size exclusions require ordered typed reasons")


TaskSizeDecision: TypeAlias = TaskSizeSample | TaskSizeExclusion


@dataclass(frozen=True, slots=True)
class TaskSizeAccounting:
    """Counts each candidate terminal Observation once, regardless of reasons."""

    candidate_observations: int
    eligible_observations: int
    excluded_observations: int

    def __post_init__(self) -> None:
        counts = (
            self.candidate_observations,
            self.eligible_observations,
            self.excluded_observations,
        )
        if any(
            isinstance(count, bool)
            or not isinstance(count, int)
            or count < 0
            or count > MAXIMUM_SAFE_TASK_COUNT
            for count in counts
        ):
            raise WorkloadDatasetError(
                "task-size accounting counts must be safe non-negative integers"
            )
        if self.candidate_observations != (self.eligible_observations + self.excluded_observations):
            raise WorkloadDatasetError(
                "candidate observations must equal eligible plus excluded observations"
            )


class TaskSizeDistributionStatus(StrEnum):
    AVAILABLE = "available"
    INSUFFICIENT = "insufficient"


@dataclass(frozen=True, slots=True)
class TaskSizeDistribution:
    """Unweighted Type-7 task-size quantiles and a complete row decision ledger."""

    method_version: str
    configuration: ConfigurationKey
    status: TaskSizeDistributionStatus
    p25_raw_tokens: float | None
    p50_raw_tokens: float | None
    p75_raw_tokens: float | None
    accounting: TaskSizeAccounting
    decisions: tuple[TaskSizeDecision, ...]

    def __post_init__(self) -> None:
        if self.method_version != WORKLOAD_METHOD_VERSION:
            raise WorkloadAnalysisError("task-size method version must be P022 version 1.0.0")
        _validate_configuration(self.configuration)
        if not isinstance(self.status, TaskSizeDistributionStatus):
            raise WorkloadAnalysisError("task-size distribution status must be typed")
        if not isinstance(self.accounting, TaskSizeAccounting):
            raise WorkloadAnalysisError("task-size distribution accounting must be typed")
        if not isinstance(self.decisions, tuple) or any(
            not isinstance(decision, (TaskSizeSample, TaskSizeExclusion))
            for decision in self.decisions
        ):
            raise WorkloadAnalysisError("task-size decisions must be an immutable typed tuple")
        if len(self.decisions) > MAXIMUM_WORKLOAD_OBSERVATIONS:
            raise WorkloadDatasetError("task-size decisions exceed the P017 reader safety limit")
        decision_ids = tuple(decision.observation_id for decision in self.decisions)
        if len(set(decision_ids)) != len(decision_ids) or decision_ids != tuple(
            sorted(decision_ids)
        ):
            raise WorkloadAnalysisError("task-size decisions must have unique sorted IDs")
        samples = self.samples
        exclusions = self.exclusions
        if any(sample.configuration != self.configuration for sample in samples):
            raise WorkloadConfigurationMismatchError(
                "task-size sample configuration differs from its distribution"
            )
        if self.accounting != TaskSizeAccounting(
            len(self.decisions), len(samples), len(exclusions)
        ):
            raise WorkloadAnalysisError("task-size accounting does not match its decisions")

        quantiles = (self.p25_raw_tokens, self.p50_raw_tokens, self.p75_raw_tokens)
        if len(samples) < MINIMUM_TASK_SIZE_SAMPLES:
            if self.status is not TaskSizeDistributionStatus.INSUFFICIENT or any(
                value is not None for value in quantiles
            ):
                raise WorkloadAnalysisError(
                    "fewer than three eligible tasks must omit task-size quantiles"
                )
            return
        if self.status is not TaskSizeDistributionStatus.AVAILABLE or any(
            value is None for value in quantiles
        ):
            raise WorkloadAnalysisError(
                "three or more eligible tasks require available task-size quantiles"
            )
        normalized = tuple(_finite_float(value, "task-size quantile") for value in quantiles)
        if any(value <= 0 for value in normalized):
            raise WorkloadAnalysisError("task-size quantiles must be positive")
        if not normalized[0] <= normalized[1] <= normalized[2]:
            raise WorkloadAnalysisError("task-size quantiles must be ordered")
        object.__setattr__(self, "p25_raw_tokens", normalized[0])
        object.__setattr__(self, "p50_raw_tokens", normalized[1])
        object.__setattr__(self, "p75_raw_tokens", normalized[2])

    @property
    def samples(self) -> tuple[TaskSizeSample, ...]:
        return tuple(
            decision for decision in self.decisions if isinstance(decision, TaskSizeSample)
        )

    @property
    def exclusions(self) -> tuple[TaskSizeExclusion, ...]:
        return tuple(
            decision for decision in self.decisions if isinstance(decision, TaskSizeExclusion)
        )

    @property
    def c_quality_observations(self) -> int:
        """Number of included C-quality tasks, which may contain concurrent usage."""

        return sum(sample.token_quality is QualityGrade.C for sample in self.samples)


class WorkloadInsufficiencyReason(StrEnum):
    TASK_HISTORY_INSUFFICIENT = "task_history_insufficient"
    CAPACITY_INSUFFICIENT = "capacity_insufficient"


class WorkloadPredictionStatus(StrEnum):
    AVAILABLE = "available"
    INSUFFICIENT_TASK_HISTORY = "insufficient_task_history"
    INSUFFICIENT_CAPACITY = "insufficient_capacity"
    INSUFFICIENT_TASK_HISTORY_AND_CAPACITY = "insufficient_task_history_and_capacity"


@dataclass(frozen=True, slots=True)
class MeterWorkloadPrediction:
    """Independent scenario counts for one P020 meter estimate."""

    meter_type: QuotaMeterType
    configuration: ConfigurationKey
    capacity_confidence: Confidence
    capacity_provider_authority: str
    capacity_reason_codes: tuple[ReasonCode, ...]
    estimated_remaining_raw_tokens: float | None
    light_task_count: int | None
    normal_task_count: int | None
    heavy_task_count: int | None
    status: WorkloadPredictionStatus
    insufficiency_reasons: tuple[WorkloadInsufficiencyReason, ...]

    def __post_init__(self) -> None:
        if not isinstance(self.meter_type, QuotaMeterType):
            raise WorkloadAnalysisError("workload meter type must be typed")
        _validate_configuration(self.configuration)
        if not isinstance(self.capacity_confidence, Confidence):
            raise WorkloadAnalysisError("capacity confidence must be a P020 confidence value")
        if self.capacity_provider_authority != PROVIDER_AUTHORITY:
            raise WorkloadAnalysisError("capacity provider authority must preserve P020 metadata")
        if (
            not isinstance(self.capacity_reason_codes, tuple)
            or any(not isinstance(reason, ReasonCode) for reason in self.capacity_reason_codes)
            or self.capacity_reason_codes
            != tuple(
                reason for reason in _REASON_CODE_ORDER if reason in self.capacity_reason_codes
            )
        ):
            raise WorkloadAnalysisError("capacity reason codes must be typed and ordered")
        if not isinstance(self.status, WorkloadPredictionStatus):
            raise WorkloadAnalysisError("workload prediction status must be typed")
        if (
            not isinstance(self.insufficiency_reasons, tuple)
            or any(
                not isinstance(reason, WorkloadInsufficiencyReason)
                for reason in self.insufficiency_reasons
            )
            or self.insufficiency_reasons
            != tuple(
                reason
                for reason in WorkloadInsufficiencyReason
                if reason in self.insufficiency_reasons
            )
        ):
            raise WorkloadAnalysisError("workload insufficiency reasons must be typed and ordered")

        counts = (self.light_task_count, self.normal_task_count, self.heavy_task_count)
        if self.capacity_confidence is Confidence.INSUFFICIENT:
            if self.estimated_remaining_raw_tokens is not None:
                raise WorkloadAnalysisError(
                    "insufficient P020 capacity cannot expose remaining raw tokens"
                )
            if not self.capacity_reason_codes:
                raise WorkloadAnalysisError("insufficient P020 capacity must retain its reasons")
        else:
            if self.capacity_confidence not in _SUFFICIENT_CONFIDENCE:
                raise WorkloadAnalysisError("unsupported P020 capacity confidence")
            if self.estimated_remaining_raw_tokens is None:
                raise WorkloadAnalysisError("sufficient capacity must retain remaining raw tokens")
            remaining = _finite_float(
                self.estimated_remaining_raw_tokens,
                "estimated remaining raw tokens",
                nonnegative=True,
            )
            object.__setattr__(self, "estimated_remaining_raw_tokens", remaining)

        if self.status is WorkloadPredictionStatus.AVAILABLE:
            if self.insufficiency_reasons or self.capacity_confidence is Confidence.INSUFFICIENT:
                raise WorkloadAnalysisError(
                    "available predictions cannot have insufficient evidence"
                )
            if any(
                isinstance(count, bool)
                or not isinstance(count, int)
                or not 0 <= count <= MAXIMUM_SAFE_TASK_COUNT
                for count in counts
            ):
                raise WorkloadAnalysisError(
                    "available task counts must be safe non-negative integers"
                )
            assert self.light_task_count is not None
            assert self.normal_task_count is not None
            assert self.heavy_task_count is not None
            if not self.light_task_count >= self.normal_task_count >= self.heavy_task_count:
                raise WorkloadAnalysisError(
                    "task counts must decrease as task-size quantiles increase"
                )
            return

        if any(count is not None for count in counts):
            raise WorkloadAnalysisError("unavailable task counts must be None")
        expected_status_reasons = {
            WorkloadPredictionStatus.INSUFFICIENT_TASK_HISTORY: (
                WorkloadInsufficiencyReason.TASK_HISTORY_INSUFFICIENT,
            ),
            WorkloadPredictionStatus.INSUFFICIENT_CAPACITY: (
                WorkloadInsufficiencyReason.CAPACITY_INSUFFICIENT,
            ),
            WorkloadPredictionStatus.INSUFFICIENT_TASK_HISTORY_AND_CAPACITY: (
                WorkloadInsufficiencyReason.TASK_HISTORY_INSUFFICIENT,
                WorkloadInsufficiencyReason.CAPACITY_INSUFFICIENT,
            ),
        }.get(self.status)
        if expected_status_reasons is None or self.insufficiency_reasons != expected_status_reasons:
            raise WorkloadAnalysisError("workload status and insufficiency reasons disagree")
        has_insufficient_capacity = (
            WorkloadInsufficiencyReason.CAPACITY_INSUFFICIENT in self.insufficiency_reasons
        )
        if has_insufficient_capacity != (self.capacity_confidence is Confidence.INSUFFICIENT):
            raise WorkloadAnalysisError("workload status does not match P020 capacity confidence")


@dataclass(frozen=True, slots=True)
class RemainingWorkloadPrediction:
    """Internal typed P022 result, separate from the AnalyticsResult v1 contract."""

    method_version: str
    configuration: ConfigurationKey
    task_size_distribution: TaskSizeDistribution
    meter_predictions: tuple[MeterWorkloadPrediction, ...]
    capacity_request_id: str
    capacity_generated_at: str
    capacity_estimator_method_version: str

    def __post_init__(self) -> None:
        if self.method_version != WORKLOAD_METHOD_VERSION:
            raise WorkloadAnalysisError("workload method version must be P022 version 1.0.0")
        _validate_configuration(self.configuration)
        if not isinstance(self.task_size_distribution, TaskSizeDistribution):
            raise WorkloadAnalysisError("workload result requires a typed task-size distribution")
        if self.task_size_distribution.configuration != self.configuration:
            raise WorkloadConfigurationMismatchError(
                "workload configuration differs from task-size history"
            )
        if not isinstance(self.meter_predictions, tuple) or any(
            not isinstance(prediction, MeterWorkloadPrediction)
            for prediction in self.meter_predictions
        ):
            raise WorkloadAnalysisError("meter workload predictions must be a typed tuple")
        if len({prediction.meter_type for prediction in self.meter_predictions}) != len(
            self.meter_predictions
        ):
            raise WorkloadAnalysisError("meter workload predictions must be unique")
        if any(
            prediction.configuration != self.configuration for prediction in self.meter_predictions
        ):
            raise WorkloadConfigurationMismatchError(
                "meter workload configuration differs from the result"
            )
        if (
            not isinstance(self.capacity_request_id, str)
            or not self.capacity_request_id
            or not isinstance(self.capacity_generated_at, str)
            or not self.capacity_generated_at
            or self.capacity_estimator_method_version != ESTIMATOR_METHOD_VERSION
        ):
            raise WorkloadCapacityError("P020 capacity provenance must remain typed and complete")
        for prediction in self.meter_predictions:
            _validate_prediction_against_distribution(prediction, self.task_size_distribution)


def _validate_prediction_against_distribution(
    prediction: MeterWorkloadPrediction,
    distribution: TaskSizeDistribution,
) -> None:
    task_history_insufficient = distribution.status is TaskSizeDistributionStatus.INSUFFICIENT
    capacity_insufficient = prediction.capacity_confidence is Confidence.INSUFFICIENT
    if task_history_insufficient and capacity_insufficient:
        expected_status = WorkloadPredictionStatus.INSUFFICIENT_TASK_HISTORY_AND_CAPACITY
    elif task_history_insufficient:
        expected_status = WorkloadPredictionStatus.INSUFFICIENT_TASK_HISTORY
    elif capacity_insufficient:
        expected_status = WorkloadPredictionStatus.INSUFFICIENT_CAPACITY
    else:
        expected_status = WorkloadPredictionStatus.AVAILABLE
    if prediction.status is not expected_status:
        raise WorkloadAnalysisError("meter status does not match its capacity and task evidence")
    if expected_status is not WorkloadPredictionStatus.AVAILABLE:
        return

    assert distribution.p25_raw_tokens is not None
    assert distribution.p50_raw_tokens is not None
    assert distribution.p75_raw_tokens is not None
    assert prediction.estimated_remaining_raw_tokens is not None
    expected_counts = tuple(
        _task_count(prediction.estimated_remaining_raw_tokens, task_size)
        for task_size in (
            distribution.p25_raw_tokens,
            distribution.p50_raw_tokens,
            distribution.p75_raw_tokens,
        )
    )
    if expected_counts != (
        prediction.light_task_count,
        prediction.normal_task_count,
        prediction.heavy_task_count,
    ):
        raise WorkloadAnalysisError("task counts do not match the P020 capacity and task quantiles")


def _task_count(remaining_raw_tokens: float, task_raw_tokens: float) -> int:
    remaining = _finite_float(
        remaining_raw_tokens,
        "estimated remaining raw tokens",
        nonnegative=True,
    )
    task_size = _finite_float(task_raw_tokens, "historical task size")
    if task_size <= 0:
        raise WorkloadNumericalError("historical task size must be positive")
    if remaining == 0.0:
        return 0
    try:
        ratio = remaining / task_size
    except (OverflowError, ZeroDivisionError) as exc:
        raise WorkloadNumericalError("remaining task-count division failed") from exc
    if not math.isfinite(ratio):
        raise WorkloadNumericalError("remaining task-count division is not finite")
    try:
        count = math.floor(ratio)
    except (OverflowError, ValueError) as exc:
        raise WorkloadNumericalError("remaining task count cannot be represented") from exc
    if count < 0 or count > MAXIMUM_SAFE_TASK_COUNT:
        raise WorkloadNumericalError("remaining task count exceeds the v1 safe integer bound")
    return count


def _type7_quantile(values: tuple[int, ...], probability: float) -> float:
    position = (len(values) - 1) * probability
    lower = math.floor(position)
    upper = math.ceil(position)
    fraction = position - lower
    try:
        lower_value = float(values[lower])
        upper_value = float(values[upper])
    except (OverflowError, ValueError) as exc:
        raise WorkloadNumericalError(
            "historical task-size quantile is outside numeric range"
        ) from exc
    if not math.isfinite(lower_value) or not math.isfinite(upper_value):
        raise WorkloadNumericalError("historical task-size quantile is not finite")
    # This is algebraically equivalent to Type-7's weighted sum and avoids
    # overflowing an intermediate sum for large, finite endpoint values.
    quantile = lower_value + (upper_value - lower_value) * fraction
    if not math.isfinite(quantile) or quantile <= 0:
        raise WorkloadNumericalError("historical task-size quantile is not positive and finite")
    return quantile


def _validate_capacity_result(
    capacity_result: AnalyticsResultV1,
) -> tuple[ConfigurationKey, tuple[QuotaCapacityEstimate, ...]]:
    if not isinstance(capacity_result, AnalyticsResultV1):
        raise WorkloadCapacityError("workload prediction requires a validated AnalyticsResultV1")
    identity = capacity_result.dataset_configuration
    if not isinstance(identity, ResultConfigurationIdentity) or any(
        not isinstance(value, ResultConfigurationValue)
        for value in (
            identity.plan,
            identity.model,
            identity.reasoning_level,
            identity.speed_mode,
            identity.codex_version,
        )
    ):
        raise WorkloadCapacityError("P020 result configuration identity is invalid")
    configuration = identity.configuration_key
    try:
        _validate_configuration(configuration)
    except WorkloadDatasetError as exc:
        raise WorkloadCapacityError("P020 result configuration identity is invalid") from exc
    if (
        not isinstance(capacity_result.request_id, str)
        or not capacity_result.request_id
        or not isinstance(capacity_result.result_status, AnalyticsResultStatus)
        or capacity_result.estimator_method_version != ESTIMATOR_METHOD_VERSION
        or not isinstance(capacity_result.generated_at, str)
        or not capacity_result.generated_at
        or not isinstance(capacity_result.quota_capacity_estimates, tuple)
        or any(
            not isinstance(estimate, QuotaCapacityEstimate)
            for estimate in capacity_result.quota_capacity_estimates
        )
    ):
        raise WorkloadCapacityError("P020 capacity result fields are invalid")
    estimates = capacity_result.quota_capacity_estimates
    if len({estimate.meter_type for estimate in estimates}) != len(estimates):
        raise WorkloadCapacityError("P020 capacity result meters must be unique")
    has_sufficient = False
    for estimate in estimates:
        if not isinstance(estimate.meter_type, QuotaMeterType) or not isinstance(
            estimate.confidence, Confidence
        ):
            raise WorkloadCapacityError("P020 capacity meter identity and confidence must be typed")
        if estimate.confidence is Confidence.INSUFFICIENT:
            numbers = (
                estimate.estimated_full_capacity_raw_tokens,
                estimate.estimated_used_raw_tokens,
                estimate.estimated_remaining_raw_tokens,
                estimate.p25_raw_tokens,
                estimate.p50_raw_tokens,
                estimate.p75_raw_tokens,
            )
            if any(number is not None for number in numbers) or not estimate.reason_codes:
                raise WorkloadCapacityError("insufficient P020 capacity fields are inconsistent")
        elif estimate.confidence in _SUFFICIENT_CONFIDENCE:
            has_sufficient = True
            _finite_float(
                estimate.estimated_remaining_raw_tokens,
                "P020 estimated remaining raw tokens",
                nonnegative=True,
            )
        else:
            raise WorkloadCapacityError("P020 capacity confidence is unsupported")
    expected_status = (
        AnalyticsResultStatus.SUCCEEDED
        if has_sufficient
        else AnalyticsResultStatus.INSUFFICIENT_EVIDENCE
    )
    if capacity_result.result_status is not expected_status:
        raise WorkloadCapacityError("P020 result status does not match meter confidence")
    return configuration, estimates


def _validate_dataset(
    dataset: ObservationDataset,
    target_configuration: ConfigurationKey,
) -> tuple[ObservationRecord, ...]:
    if not isinstance(dataset, ObservationDataset) or not isinstance(dataset.observations, tuple):
        raise WorkloadDatasetError("workload prediction requires an immutable ObservationDataset")
    if len(dataset.observations) > MAXIMUM_WORKLOAD_OBSERVATIONS:
        raise WorkloadDatasetError(
            "dataset exceeds the P017 reader safety limit; select a bounded dataset with P017"
        )
    selected_configuration = dataset.selected_configuration
    if selected_configuration is not None:
        _validate_configuration(selected_configuration)
        if selected_configuration != target_configuration:
            raise WorkloadConfigurationMismatchError(
                "selected task history configuration differs from the P020 target"
            )
    seen: set[str] = set()
    for record in dataset.observations:
        if not isinstance(record, ObservationRecord):
            raise WorkloadDatasetError("task history must contain typed Observation records")
        if not isinstance(record.observation_id, str) or not record.observation_id:
            raise WorkloadDatasetError("task history Observation ID must be non-empty")
        if record.observation_id in seen:
            raise WorkloadDatasetError("duplicate task history Observation ID")
        seen.add(record.observation_id)
        if not isinstance(record.lifecycle, ObservationLifecycle) or record.lifecycle not in (
            ObservationLifecycle.FINALIZED,
            ObservationLifecycle.INCOMPLETE,
            ObservationLifecycle.INVALID,
        ):
            raise WorkloadDatasetError("workload prediction accepts terminal Observations only")
        if any(
            value is not None and not isinstance(value, str)
            for value in (record.started_at, record.ended_at, record.finalized_at)
        ):
            raise WorkloadDatasetError("Observation timestamps must be strings or None")
        _validate_configuration(record.configuration)
        if record.configuration != target_configuration:
            raise WorkloadConfigurationMismatchError(
                "task history configuration differs from the P020 target"
            )
        if not isinstance(record.summary_quality, QualityGrade):
            raise WorkloadDatasetError("Observation summary quality must be typed")
        if not isinstance(record.token_validity, EvidenceValidity):
            raise WorkloadDatasetError("token validity must use a typed evidence state")
        if not isinstance(record.token_quality, QualityGrade):
            raise WorkloadDatasetError("token quality must use a typed quality grade")
        if record.raw_total is not None and (
            isinstance(record.raw_total, bool)
            or not isinstance(record.raw_total, int)
            or record.raw_total < 0
        ):
            raise WorkloadDatasetError("raw total must be None or a non-negative integer")
    return dataset.observations


def _task_size_decision(record: ObservationRecord) -> TaskSizeDecision:
    reasons: set[TaskSizeExclusionReason] = set()
    if record.token_validity is not EvidenceValidity.VALID:
        reasons.add(TaskSizeExclusionReason.TOKEN_NOT_VALID)
    if record.token_quality not in _ELIGIBLE_TOKEN_QUALITIES:
        reasons.add(TaskSizeExclusionReason.TOKEN_QUALITY_INELIGIBLE)
    if record.raw_total is None:
        reasons.add(TaskSizeExclusionReason.RAW_TOTAL_UNAVAILABLE)
    elif record.raw_total == 0:
        reasons.add(TaskSizeExclusionReason.ZERO_RAW_TOTAL)
    if reasons:
        return TaskSizeExclusion(
            record.observation_id,
            tuple(reason for reason in _TASK_SIZE_REASON_ORDER if reason in reasons),
        )
    assert record.raw_total is not None
    return TaskSizeSample(
        record.observation_id,
        record.configuration,
        record.raw_total,
        record.token_quality,
    )


def _task_size_distribution(
    dataset: ObservationDataset,
    target_configuration: ConfigurationKey,
) -> TaskSizeDistribution:
    records = _validate_dataset(dataset, target_configuration)
    decisions = tuple(
        sorted(
            (_task_size_decision(record) for record in records),
            key=lambda decision: decision.observation_id,
        )
    )
    samples = tuple(decision for decision in decisions if isinstance(decision, TaskSizeSample))
    exclusions = tuple(
        decision for decision in decisions if isinstance(decision, TaskSizeExclusion)
    )
    accounting = TaskSizeAccounting(len(records), len(samples), len(exclusions))
    if len(samples) < MINIMUM_TASK_SIZE_SAMPLES:
        return TaskSizeDistribution(
            WORKLOAD_METHOD_VERSION,
            target_configuration,
            TaskSizeDistributionStatus.INSUFFICIENT,
            None,
            None,
            None,
            accounting,
            decisions,
        )
    values = tuple(sorted(sample.raw_total for sample in samples))
    return TaskSizeDistribution(
        WORKLOAD_METHOD_VERSION,
        target_configuration,
        TaskSizeDistributionStatus.AVAILABLE,
        _type7_quantile(values, 0.25),
        _type7_quantile(values, 0.50),
        _type7_quantile(values, 0.75),
        accounting,
        decisions,
    )


def _meter_prediction(
    estimate: QuotaCapacityEstimate,
    configuration: ConfigurationKey,
    distribution: TaskSizeDistribution,
) -> MeterWorkloadPrediction:
    capacity_insufficient = estimate.confidence is Confidence.INSUFFICIENT
    task_history_insufficient = distribution.status is TaskSizeDistributionStatus.INSUFFICIENT
    insufficiency_reasons = tuple(
        reason
        for reason, applies in (
            (
                WorkloadInsufficiencyReason.TASK_HISTORY_INSUFFICIENT,
                task_history_insufficient,
            ),
            (
                WorkloadInsufficiencyReason.CAPACITY_INSUFFICIENT,
                capacity_insufficient,
            ),
        )
        if applies
    )
    if len(insufficiency_reasons) == 2:
        status = WorkloadPredictionStatus.INSUFFICIENT_TASK_HISTORY_AND_CAPACITY
    elif task_history_insufficient:
        status = WorkloadPredictionStatus.INSUFFICIENT_TASK_HISTORY
    elif capacity_insufficient:
        status = WorkloadPredictionStatus.INSUFFICIENT_CAPACITY
    else:
        status = WorkloadPredictionStatus.AVAILABLE

    remaining = estimate.estimated_remaining_raw_tokens
    if status is not WorkloadPredictionStatus.AVAILABLE:
        return MeterWorkloadPrediction(
            estimate.meter_type,
            configuration,
            estimate.confidence,
            PROVIDER_AUTHORITY,
            estimate.reason_codes,
            remaining,
            None,
            None,
            None,
            status,
            insufficiency_reasons,
        )
    assert remaining is not None
    assert distribution.p25_raw_tokens is not None
    assert distribution.p50_raw_tokens is not None
    assert distribution.p75_raw_tokens is not None
    return MeterWorkloadPrediction(
        estimate.meter_type,
        configuration,
        estimate.confidence,
        PROVIDER_AUTHORITY,
        estimate.reason_codes,
        remaining,
        _task_count(remaining, distribution.p25_raw_tokens),
        _task_count(remaining, distribution.p50_raw_tokens),
        _task_count(remaining, distribution.p75_raw_tokens),
        status,
        insufficiency_reasons,
    )


def predict_remaining_workload(
    *,
    dataset: ObservationDataset,
    capacity_result: AnalyticsResultV1,
) -> RemainingWorkloadPrediction:
    """Predict empirical light/normal/heavy task counts from P017 and P020 inputs.

    The function consumes exactly the caller-selected bounded history and the
    explicit remaining-capacity estimates supplied by P020. It does not open
    SQLite, read the clock, acquire quotas, or infer current meter positions.
    """

    configuration, estimates = _validate_capacity_result(capacity_result)
    distribution = _task_size_distribution(dataset, configuration)
    predictions = tuple(
        _meter_prediction(estimate, configuration, distribution) for estimate in estimates
    )
    return RemainingWorkloadPrediction(
        WORKLOAD_METHOD_VERSION,
        configuration,
        distribution,
        predictions,
        capacity_result.request_id,
        capacity_result.generated_at,
        capacity_result.estimator_method_version,
    )


__all__ = [
    "MAXIMUM_SAFE_TASK_COUNT",
    "MAXIMUM_WORKLOAD_OBSERVATIONS",
    "MINIMUM_TASK_SIZE_SAMPLES",
    "WORKLOAD_METHOD_VERSION",
    "MeterWorkloadPrediction",
    "RemainingWorkloadPrediction",
    "TaskSizeAccounting",
    "TaskSizeDecision",
    "TaskSizeDistribution",
    "TaskSizeDistributionStatus",
    "TaskSizeExclusion",
    "TaskSizeExclusionReason",
    "TaskSizeSample",
    "WorkloadAnalysisError",
    "WorkloadCapacityError",
    "WorkloadConfigurationMismatchError",
    "WorkloadDatasetError",
    "WorkloadInsufficiencyReason",
    "WorkloadNumericalError",
    "WorkloadPredictionStatus",
    "predict_remaining_workload",
]
