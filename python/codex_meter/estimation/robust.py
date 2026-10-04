"""Deterministic robust aggregation of per-meter capacity candidates."""

import math
from collections.abc import Mapping
from dataclasses import dataclass, replace
from enum import StrEnum
from types import MappingProxyType

from codex_meter.history import ConfigurationKey, QualityGrade, QuotaMeterType

from .candidates import MeterCandidateSet, RawCapacityCandidate, SampleAccounting

ESTIMATOR_METHOD_VERSION = "1.0.0"
MODIFIED_Z_CONSTANT = 0.6744897501960817
MODIFIED_Z_THRESHOLD = 3.5

QUALITY_WEIGHTS_V1: Mapping[QualityGrade, int] = MappingProxyType(
    {
        QualityGrade.A: 10,
        QualityGrade.B: 8,
        QualityGrade.C: 3,
    }
)


class AggregationStatus(StrEnum):
    """Availability of robust capacity statistics for a candidate set."""

    SUFFICIENT = "sufficient"
    INSUFFICIENT_SAMPLES = "insufficient_samples"


class RobustAggregationError(ValueError):
    """Base error for invalid or impossible robust aggregation input."""


class CandidateSetInvariantError(RobustAggregationError):
    """Raised when a candidate set violates P018 invariants."""


@dataclass(frozen=True, slots=True)
class RobustCapacityAggregation:
    """Immutable robust statistics for one homogeneous quota-meter candidate set."""

    estimator_method_version: str
    meter_type: QuotaMeterType
    configuration: ConfigurationKey | None
    status: AggregationStatus
    pre_filter_median_raw_tokens: float | None
    mad_raw_tokens: float | None
    weighted_median_raw_tokens: float | None
    p25_raw_tokens: float | None
    p50_raw_tokens: float | None
    p75_raw_tokens: float | None
    accounting: SampleAccounting
    used_observation_ids: tuple[str, ...]
    outlier_observation_ids: tuple[str, ...]

    @property
    def unweighted_median_raw_tokens(self) -> float | None:
        """Backward-compatible explicit name for the pre-filter ordinary median."""

        return self.pre_filter_median_raw_tokens


def _median(values: tuple[float, ...] | list[float]) -> float:
    if not values:
        raise RobustAggregationError("median requires at least one value")
    ordered = sorted(values)
    middle = len(ordered) // 2
    if len(ordered) % 2:
        return ordered[middle]
    return (ordered[middle - 1] + ordered[middle]) / 2


def _quantile(values: tuple[float, ...], probability: float) -> float:
    if not values:
        raise RobustAggregationError("quantile requires at least one value")
    ordered = sorted(values)
    position = (len(ordered) - 1) * probability
    lower = math.floor(position)
    upper = math.ceil(position)
    fraction = position - lower
    return ordered[lower] * (1 - fraction) + ordered[upper] * fraction


def _weighted_median(candidates: tuple[RawCapacityCandidate, ...]) -> float:
    if not candidates:
        raise RobustAggregationError("weighted median requires at least one candidate")
    ordered = sorted(
        candidates,
        key=lambda candidate: (candidate.full_capacity_raw_tokens, candidate.observation_id),
    )
    total_weight = sum(QUALITY_WEIGHTS_V1[candidate.quality] for candidate in ordered)
    cumulative_weight = 0
    for index, candidate in enumerate(ordered):
        cumulative_weight += QUALITY_WEIGHTS_V1[candidate.quality]
        doubled_cumulative = 2 * cumulative_weight
        if doubled_cumulative > total_weight:
            return candidate.full_capacity_raw_tokens
        if doubled_cumulative == total_weight:
            if index + 1 < len(ordered):
                return (
                    candidate.full_capacity_raw_tokens + ordered[index + 1].full_capacity_raw_tokens
                ) / 2
            return candidate.full_capacity_raw_tokens
    raise RobustAggregationError("weighted median did not identify a candidate")


def _validate_finite_positive(value: float, label: str) -> None:
    if not math.isfinite(value) or value <= 0:
        raise CandidateSetInvariantError(f"{label} must be finite and positive")


def _validate_candidate_set(candidate_set: MeterCandidateSet) -> None:
    decisions = candidate_set.decisions
    candidates = candidate_set.candidates
    accounting = candidate_set.accounting

    if len(decisions) != accounting.candidate_observations:
        raise CandidateSetInvariantError(
            "decision count must equal accounting candidate observations"
        )
    if len(candidates) != accounting.valid_observations:
        raise CandidateSetInvariantError("candidate count must equal accounting valid observations")
    if (
        accounting.outliers_removed != 0
        or accounting.used_observations != accounting.valid_observations
    ):
        raise CandidateSetInvariantError(
            "input candidate accounting must be pre-outlier accounting"
        )

    decision_ids = [decision.observation_id for decision in decisions]
    if len(decision_ids) != len(set(decision_ids)):
        raise CandidateSetInvariantError("observation IDs must be unique")
    if any(decision.meter_type is not candidate_set.meter_type for decision in decisions):
        raise CandidateSetInvariantError("decision meter type must match candidate-set meter type")

    decision_candidates = tuple(
        decision.candidate for decision in decisions if decision.candidate is not None
    )
    if decision_candidates != candidates:
        raise CandidateSetInvariantError(
            "candidate tuple must match included decision candidates in chronological order"
        )
    if len(decision_candidates) != accounting.valid_observations:
        raise CandidateSetInvariantError("included decision count must equal valid observations")
    if accounting.excluded_observations != len(decisions) - len(decision_candidates):
        raise CandidateSetInvariantError("excluded observations do not match decisions")
    if any(
        decision.candidate is not None
        and decision.observation_id != decision.candidate.observation_id
        for decision in decisions
    ):
        raise CandidateSetInvariantError(
            "included decision IDs must match candidate observation IDs"
        )

    candidate_ids = [candidate.observation_id for candidate in candidates]
    if len(candidate_ids) != len(set(candidate_ids)):
        raise CandidateSetInvariantError("included observation IDs must be unique")

    for candidate in candidates:
        if candidate.meter_type is not candidate_set.meter_type:
            raise CandidateSetInvariantError("candidate meter type must match candidate set")
        if candidate.configuration != candidate_set.configuration:
            raise CandidateSetInvariantError("candidate configuration must match candidate set")
        if candidate.quality not in QUALITY_WEIGHTS_V1:
            raise CandidateSetInvariantError("candidate quality must be A, B, or C")
        _validate_finite_positive(
            candidate.full_capacity_raw_tokens, "candidate full capacity raw tokens"
        )


def _outlier_ids(
    candidates: tuple[RawCapacityCandidate, ...],
    pre_filter_median: float,
    mad: float,
) -> tuple[str, ...]:
    if len(candidates) < 3:
        return ()

    if mad == 0:
        return tuple(
            candidate.observation_id
            for candidate in candidates
            if candidate.full_capacity_raw_tokens != pre_filter_median
        )

    return tuple(
        candidate.observation_id
        for candidate in candidates
        if MODIFIED_Z_CONSTANT * abs(candidate.full_capacity_raw_tokens - pre_filter_median) / mad
        > MODIFIED_Z_THRESHOLD
    )


def _stats_for_candidates(
    candidates: tuple[RawCapacityCandidate, ...],
) -> tuple[float, float, float, float]:
    values = tuple(candidate.full_capacity_raw_tokens for candidate in candidates)
    p25 = _quantile(values, 0.25)
    p50 = _median(values)
    p75 = _quantile(values, 0.75)
    weighted_median = _weighted_median(candidates)
    return weighted_median, p25, p50, p75


def _validate_output(values: tuple[float | None, ...]) -> None:
    if any(value is not None and not math.isfinite(value) for value in values):
        raise RobustAggregationError("robust aggregation produced a non-finite statistic")


def aggregate_capacity_candidates(
    candidate_set: MeterCandidateSet,
) -> RobustCapacityAggregation:
    """Aggregate one P018 candidate set without accessing storage or other meters."""

    _validate_candidate_set(candidate_set)
    candidates = candidate_set.candidates
    if len(candidates) < 2:
        accounting = replace(
            candidate_set.accounting,
            outliers_removed=0,
            used_observations=len(candidates),
        )
        return RobustCapacityAggregation(
            estimator_method_version=ESTIMATOR_METHOD_VERSION,
            meter_type=candidate_set.meter_type,
            configuration=candidate_set.configuration,
            status=AggregationStatus.INSUFFICIENT_SAMPLES,
            pre_filter_median_raw_tokens=None,
            mad_raw_tokens=None,
            weighted_median_raw_tokens=None,
            p25_raw_tokens=None,
            p50_raw_tokens=None,
            p75_raw_tokens=None,
            accounting=accounting,
            used_observation_ids=tuple(candidate.observation_id for candidate in candidates),
            outlier_observation_ids=(),
        )

    values = tuple(candidate.full_capacity_raw_tokens for candidate in candidates)
    pre_filter_median = _median(values)
    mad = _median(tuple(abs(value - pre_filter_median) for value in values))
    outlier_ids = _outlier_ids(candidates, pre_filter_median, mad)
    outlier_id_set = set(outlier_ids)
    retained = tuple(
        candidate for candidate in candidates if candidate.observation_id not in outlier_id_set
    )
    if not retained:
        raise RobustAggregationError("outlier filtering removed every candidate")

    accounting = replace(
        candidate_set.accounting,
        outliers_removed=len(outlier_ids),
        used_observations=len(retained),
    )
    if len(retained) < 2:
        _validate_output((pre_filter_median, mad))
        return RobustCapacityAggregation(
            estimator_method_version=ESTIMATOR_METHOD_VERSION,
            meter_type=candidate_set.meter_type,
            configuration=candidate_set.configuration,
            status=AggregationStatus.INSUFFICIENT_SAMPLES,
            pre_filter_median_raw_tokens=pre_filter_median,
            mad_raw_tokens=mad,
            weighted_median_raw_tokens=None,
            p25_raw_tokens=None,
            p50_raw_tokens=None,
            p75_raw_tokens=None,
            accounting=accounting,
            used_observation_ids=tuple(candidate.observation_id for candidate in retained),
            outlier_observation_ids=outlier_ids,
        )

    weighted_median, p25, p50, p75 = _stats_for_candidates(retained)
    _validate_output((pre_filter_median, mad, weighted_median, p25, p50, p75))
    return RobustCapacityAggregation(
        estimator_method_version=ESTIMATOR_METHOD_VERSION,
        meter_type=candidate_set.meter_type,
        configuration=candidate_set.configuration,
        status=AggregationStatus.SUFFICIENT,
        pre_filter_median_raw_tokens=pre_filter_median,
        mad_raw_tokens=mad,
        weighted_median_raw_tokens=weighted_median,
        p25_raw_tokens=p25,
        p50_raw_tokens=p50,
        p75_raw_tokens=p75,
        accounting=accounting,
        used_observation_ids=tuple(candidate.observation_id for candidate in retained),
        outlier_observation_ids=outlier_ids,
    )


__all__ = [
    "AggregationStatus",
    "CandidateSetInvariantError",
    "ESTIMATOR_METHOD_VERSION",
    "MODIFIED_Z_CONSTANT",
    "MODIFIED_Z_THRESHOLD",
    "QUALITY_WEIGHTS_V1",
    "RobustAggregationError",
    "RobustCapacityAggregation",
    "aggregate_capacity_candidates",
]
